//! Instance shutdown is separate from viewing/attaching a page. Never retry an
//! ambiguous Browser.close with a signal: the instance may be checkpointing.

use super::{process, registry::Record};
use crate::{Error, Result};

/// Return true only when the original kernel process is already inactive.
pub(super) fn close(record: &Record) -> Result<bool> {
    let active = process::is_current(&record.process)?;
    if !active {
        return Ok(true);
    }
    if record.closed {
        return Err(Error::Conflict(
            "Moli record is closed but its process is still active; no close or signal was sent"
                .into(),
        ));
    }
    #[cfg(target_os = "macos")]
    macos_close(record)?;
    // Linux retains its existing pidfd-backed signal path until equivalent
    // listener ownership verification is implemented. Windows stays unsupported.
    #[cfg(not(target_os = "macos"))]
    process::stop(&record.process)?;
    Ok(false)
}

#[cfg(target_os = "macos")]
fn macos_close(record: &Record) -> Result<()> {
    use std::time::{Duration, Instant};

    let deadline = Instant::now() + Duration::from_secs(8);
    super::validate_endpoints(&record.origin, &record.browser_ws)?;
    let port = url::Url::parse(&record.origin)
        .ok()
        .and_then(|url| url.port())
        .filter(|port| *port != 0)
        .ok_or_else(|| Error::Config("Invalid owned Moli listener".into()))?;
    process::require_listener(&record.process, port, deadline)?;
    // This transport does not redirect, attach a page, create a target, or
    // disable a selected proxy. Its I/O deadline survives the WS upgrade.
    let budget = deadline
        .checked_duration_since(Instant::now())
        .filter(|budget| !budget.is_zero())
        .ok_or_else(shutdown_timeout)?
        .min(Duration::from_secs(2));
    let mut socket = match crate::cdp::proxy::connect_owned_control(&record.browser_ws, budget) {
        Ok(socket) => socket,
        Err(error) => {
            // Failed ownership/transport inspection is not proof of exit.
            return if process::is_current(&record.process)? {
                Err(error)
            } else {
                Ok(())
            };
        }
    };
    process::require_listener(&record.process, port, deadline)?;
    let result = exchange(&mut socket);
    drop(socket);
    match result {
        Ok(CloseReply::Unsupported) => {
            // Only an exact -32601 root response permits the legacy path. The
            // stop operation independently rechecks birth identity/executable.
            if process::is_current(&record.process)? {
                process::stop_before(&record.process, deadline)?;
            }
            Ok(())
        }
        Ok(CloseReply::Accepted) | Err(_) => {
            // A write/read error can occur after the command reached Moli.
            // Waiting, without SIGTERM, gives its checkpoint the full budget.
            while process::is_current(&record.process)? {
                if Instant::now() >= deadline {
                    return Err(shutdown_timeout());
                }
                std::thread::sleep(
                    deadline
                        .saturating_duration_since(Instant::now())
                        .min(Duration::from_millis(20)),
                );
            }
            Ok(())
        }
    }
}

#[cfg(any(target_os = "macos", test))]
#[derive(Debug, PartialEq, Eq)]
enum CloseReply {
    Accepted,
    Unsupported,
}

#[cfg(any(target_os = "macos", test))]
fn exchange<S: std::io::Read + std::io::Write>(
    socket: &mut tungstenite::WebSocket<S>,
) -> Result<CloseReply> {
    use serde_json::{Value, json};
    use tungstenite::Message;

    socket
        .send(Message::text(
            json!({"id":1,"method":"Browser.close","params":{}}).to_string(),
        ))
        .map_err(|_| invalid_reply())?;
    for _ in 0..64 {
        match socket.read().map_err(|_| invalid_reply())? {
            Message::Text(text) => {
                let message: Value =
                    serde_json::from_str(text.as_ref()).map_err(|_| invalid_reply())?;
                let object = message.as_object().ok_or_else(invalid_reply)?;
                // Events may precede the one root response, but never an
                // attached-session response or a different request's result.
                if object.contains_key("sessionId") {
                    return Err(invalid_reply());
                }
                if !object.contains_key("id") {
                    if object.get("method").and_then(Value::as_str).is_none() {
                        return Err(invalid_reply());
                    }
                    continue;
                }
                return parse_reply(&message);
            }
            Message::Ping(_) | Message::Pong(_) => continue,
            _ => return Err(invalid_reply()),
        }
    }
    Err(invalid_reply())
}

#[cfg(any(target_os = "macos", test))]
fn parse_reply(message: &serde_json::Value) -> Result<CloseReply> {
    use serde_json::{Value, json};
    if message.get("id").and_then(Value::as_u64) != Some(1) || message.get("sessionId").is_some() {
        return Err(invalid_reply());
    }
    match (message.get("result"), message.get("error")) {
        (Some(result), None) if result == &json!({}) => Ok(CloseReply::Accepted),
        (None, Some(error)) if error.get("code").and_then(Value::as_i64) == Some(-32601) => {
            Ok(CloseReply::Unsupported)
        }
        _ => Err(invalid_reply()),
    }
}

#[cfg(any(target_os = "macos", test))]
fn invalid_reply() -> Error {
    Error::Cdp("Moli protocol shutdown was not acknowledged; no termination signal was sent".into())
}

#[cfg(target_os = "macos")]
fn shutdown_timeout() -> Error {
    Error::Timeout(
        "Moli protocol shutdown did not confirm process exit; no termination signal was sent"
            .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_exact_root_method_not_found_allows_signal_fallback() {
        assert_eq!(
            parse_reply(&json!({"id":1,"result":{}})).unwrap(),
            CloseReply::Accepted
        );
        assert_eq!(
            parse_reply(&json!({"id":1,"error":{"code":-32601,"message":"unsupported"}})).unwrap(),
            CloseReply::Unsupported
        );
        for reply in [
            json!({"id":2,"error":{"code":-32601}}),
            json!({"id":"1","error":{"code":-32601}}),
            json!({"id":1,"sessionId":"page","error":{"code":-32601}}),
            json!({"id":1,"result":{},"error":{"code":-32601}}),
            json!({"id":1,"error":{"code":-32000}}),
            json!({"id":1,"error":{"code":-32601.0}}),
            json!({"id":1,"result":{"accepted":true}}),
        ] {
            assert!(parse_reply(&reply).is_err());
        }
    }

    #[test]
    fn protocol_exchange_sends_one_root_command_and_does_not_retry_errors() {
        use std::{
            net::{TcpListener, TcpStream},
            time::Duration,
        };
        use tungstenite::Message;
        for accepted in [true, false] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_millis(200)))
                    .unwrap();
                let mut socket = tungstenite::accept(stream).unwrap();
                let request: serde_json::Value =
                    serde_json::from_str(socket.read().unwrap().to_text().unwrap()).unwrap();
                assert_eq!(
                    request,
                    json!({"id":1,"method":"Browser.close","params":{}})
                );
                socket
                    .send(Message::text(
                        json!({"method":"Target.targetDestroyed","params":{}}).to_string(),
                    ))
                    .unwrap();
                let reply = if accepted {
                    json!({"id":1,"result":{}})
                } else {
                    json!({"id":1,"error":{"code":-32000}})
                };
                socket.send(Message::text(reply.to_string())).unwrap();
                // The caller must not send a second command or attached-session
                // fallback after a rejection; shutdown policy belongs upstream.
                assert!(socket.read().is_err());
            });
            let stream = TcpStream::connect(address).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let (mut socket, _) = tungstenite::client(format!("ws://{address}/"), stream).unwrap();
            assert_eq!(exchange(&mut socket).is_ok(), accepted);
            drop(socket);
            server.join().unwrap();
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn accepted_or_ambiguous_close_timeout_never_signals_a_live_owned_process() {
        use super::super::{Moli, registry::Registry};
        use std::{
            net::TcpStream,
            process::{Command, Stdio},
            time::{Duration, Instant},
        };
        for mode in ["ack", "eof"] {
            let private = tempfile::tempdir().unwrap();
            let marker = private.path().join("ready");
            let mut child = OwnServer(
                Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "moli::close::tests::non_exiting_server",
                        "--ignored",
                    ])
                    .env("LEXMOUNT_TEST_MOLI_CLOSE_MODE", mode)
                    .env("LEXMOUNT_TEST_MOLI_CLOSE_READY", &marker)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap(),
            );
            let deadline = Instant::now() + Duration::from_secs(3);
            while !marker.is_file() {
                assert!(child.0.try_wait().unwrap().is_none());
                assert!(
                    Instant::now() < deadline,
                    "owned synthetic server did not start"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            let port = std::fs::read_to_string(&marker)
                .unwrap()
                .parse::<u16>()
                .unwrap();
            let id = format!("moli_{:032x}", rand::random::<u128>());
            let record = Record {
                version: 1,
                session_id: id.clone(),
                process: process::identity(child.0.id()).unwrap(),
                origin: format!("http://127.0.0.1:{port}"),
                browser_ws: format!("ws://127.0.0.1:{port}/devtools/browser/owned"),
                created_at: 1,
                closed: false,
            };
            let moli = Moli {
                registry: Registry::open(private.path().join("instances")).unwrap(),
            };
            moli.registry.write(&record).unwrap();
            let started = Instant::now();
            assert!(matches!(moli.close(&id), Err(Error::Timeout(_))));
            assert!(started.elapsed() >= Duration::from_secs(7));
            assert!(started.elapsed() < Duration::from_secs(12));
            assert!(
                marker.with_extension("responded").is_file(),
                "test must reach the one Browser.close response boundary"
            );
            assert!(
                process::is_current(&record.process).unwrap(),
                "ACK/EOF timeout must not send SIGTERM"
            );
            assert!(
                !moli.registry.read(&id).unwrap().closed,
                "failure must not persist closed state"
            );
            assert!(
                TcpStream::connect_timeout(
                    &([127, 0, 0, 1], port).into(),
                    Duration::from_millis(100)
                )
                .is_ok()
            );
            // Cleanup follows the assertions. It is a command over this test's
            // own child's stdin, never lifecycle qualification or a browser API.
            child.finish();
        }
    }

    #[cfg(target_os = "macos")]
    struct OwnServer(std::process::Child);
    #[cfg(target_os = "macos")]
    impl OwnServer {
        fn finish(&mut self) {
            use std::{
                io::Write,
                time::{Duration, Instant},
            };
            if let Some(mut stdin) = self.0.stdin.take() {
                let _ = stdin.write_all(b"x");
            }
            let deadline = Instant::now() + Duration::from_secs(2);
            while self.0.try_wait().unwrap().is_none() {
                if Instant::now() >= deadline {
                    let _ = self.0.kill();
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let _ = self.0.wait();
        }
    }
    #[cfg(target_os = "macos")]
    impl Drop for OwnServer {
        fn drop(&mut self) {
            self.finish();
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "internal synthetic server; only the owning timeout regression supplies its private paths"]
    fn non_exiting_server() {
        use std::{io::Read, net::TcpListener, path::PathBuf};
        let marker = PathBuf::from(std::env::var_os("LEXMOUNT_TEST_MOLI_CLOSE_READY").unwrap());
        let mode = std::env::var("LEXMOUNT_TEST_MOLI_CLOSE_MODE").unwrap();
        assert!(matches!(mode.as_str(), "ack" | "eof"));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        std::fs::write(&marker, listener.local_addr().unwrap().port().to_string()).unwrap();
        let mut socket = tungstenite::accept(listener.accept().unwrap().0).unwrap();
        let request: serde_json::Value =
            serde_json::from_str(socket.read().unwrap().to_text().unwrap()).unwrap();
        assert_eq!(
            request,
            json!({"id":1,"method":"Browser.close","params":{}})
        );
        if mode == "ack" {
            socket
                .send(tungstenite::Message::text(r#"{"id":1,"result":{}}"#))
                .unwrap();
        }
        drop(socket);
        std::fs::write(marker.with_extension("responded"), b"responded").unwrap();
        let _ = std::io::stdin().read_exact(&mut [0]);
        drop(listener);
    }
}
