//! Optional, best-effort CLI observations for a same-user desktop application.
//!
//! This is a side channel, never an execution gateway. It cannot change a
//! browser command's result, retry input, or introduce a network listener.

use std::{
    env,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::cdp::PointerEvent;

const MAX_FRAME_BYTES: usize = 16 * 1024;
const MAX_ID_BYTES: usize = 1024;

#[derive(Clone)]
pub struct DesktopEvents {
    inner: Arc<Mutex<Emitter>>,
}

// Deliberately not Debug: the local IPC token must never enter diagnostics.
struct Emitter {
    endpoint: PathBuf,
    token: String,
    producer_id: String,
    operation_id: String,
    connection_ref: String,
    session_id: String,
    target_id: Option<String>,
    run_id: Option<String>,
    app_instance_id: Option<String>,
    seq: u64,
}

struct AppConnection {
    endpoint: PathBuf,
    token: String,
    app_instance_id: Option<String>,
}

impl DesktopEvents {
    /// Use explicit paired IPC environment variables, or a private discovery file.
    /// Invalid configuration silently disables observations, not the operation.
    pub fn from_env(api_base_url: &str, project_id: &str, session_id: &str) -> Option<Self> {
        if env::var("LEXMOUNT_APP_EVENTS").is_ok_and(|v| v.eq_ignore_ascii_case("off")) {
            return None;
        }
        let explicit_endpoint = env::var_os("LEXMOUNT_APP_EVENT_ENDPOINT");
        let explicit_token = env::var_os("LEXMOUNT_APP_EVENT_TOKEN");
        let connection = match (explicit_endpoint, explicit_token) {
            (Some(endpoint), Some(token)) => AppConnection {
                endpoint: PathBuf::from(endpoint),
                token: token.into_string().ok()?,
                app_instance_id: None,
            },
            (None, None) => discover_connection()?,
            // Never combine half an explicit configuration with a discovered
            // token/endpoint from another app instance.
            _ => return None,
        };
        if !valid_connection(&connection)
            || session_id.is_empty()
            || session_id.len() > MAX_ID_BYTES
        {
            return None;
        }
        let connection_ref = connection_ref(api_base_url, project_id)?;
        let events = Self::new(
            connection.endpoint,
            connection.token,
            connection_ref,
            session_id.to_owned(),
            env::var("LEXMOUNT_AGENT_RUN_ID")
                .ok()
                .filter(|id| !id.is_empty() && id.len() <= 128),
        );
        if let Ok(mut emitter) = events.inner.lock() {
            emitter.app_instance_id = connection.app_instance_id;
        }
        Some(events)
    }

    fn new(
        endpoint: PathBuf,
        token: String,
        connection_ref: String,
        session_id: String,
        run_id: Option<String>,
    ) -> Self {
        static PRODUCER_ID: OnceLock<String> = OnceLock::new();
        Self {
            inner: Arc::new(Mutex::new(Emitter {
                endpoint,
                token,
                producer_id: PRODUCER_ID.get_or_init(random_id).clone(),
                operation_id: random_id(),
                connection_ref,
                session_id,
                target_id: None,
                run_id,
                app_instance_id: None,
                seq: 0,
            })),
        }
    }

    /// Explicit Agent report, addressed to an exact page and run. This does not
    /// connect to CDP or claim the requested Target still exists; the receiver
    /// must match it against its current Session/Target registry.
    pub fn from_env_for_task(
        api_base_url: &str,
        project_id: &str,
        session_id: &str,
        target_id: &str,
        run_id: &str,
    ) -> Option<Self> {
        if target_id.trim().is_empty()
            || target_id.len() > MAX_ID_BYTES
            || run_id.trim().is_empty()
            || run_id.len() > 128
            || target_id.chars().any(char::is_control)
            || run_id.chars().any(char::is_control)
        {
            return None;
        }
        let events = Self::from_env(api_base_url, project_id, session_id)?;
        if let Ok(mut emitter) = events.inner.lock() {
            emitter.target_id = Some(target_id.to_owned());
            emitter.run_id = Some(run_id.to_owned());
        }
        Some(events)
    }

    /// Send an explicit task state observation. True means a best-effort send
    /// was attempted, not delivered, persisted or accepted by any server.
    pub fn task_state(
        &self,
        state: &'static str,
        state_version: u64,
        handoff_id: Option<&str>,
    ) -> bool {
        if state_version == 0
            || !matches!(state, "running" | "needs_human" | "completed" | "failed")
            || (state == "needs_human") != handoff_id.is_some()
            || handoff_id.is_some_and(|id| {
                id.trim().is_empty() || id.len() > 128 || id.chars().any(char::is_control)
            })
        {
            return false;
        }
        let Ok(mut emitter) = self.inner.lock() else {
            return false;
        };
        if emitter.target_id.is_none() || emitter.run_id.is_none() {
            return false;
        }
        emitter.emit(
            "task.state",
            json!({"state":state,"state_version":state_version,"handoff_id":handoff_id}),
        );
        true
    }

    pub fn command_started(&self, action: &'static str) {
        self.emit("command.started", json!({"action":action}));
    }

    /// Bind only an ID obtained from Cdp::target_id(), never the requested flag.
    pub fn target_resolved(&self, target_id: &str) {
        if target_id.is_empty() || target_id.len() > MAX_ID_BYTES {
            return;
        }
        if let Ok(mut emitter) = self.inner.lock() {
            emitter.target_id = Some(target_id.to_owned());
            emitter.emit("command.target", json!({}));
        }
    }

    pub fn pointer(&self, pointer: &PointerEvent) {
        let kind = match pointer.phase {
            "move" => "pointer.move",
            "down" => "pointer.down",
            "up" => "pointer.up",
            _ => return,
        };
        if let Ok(payload) = serde_json::to_value(pointer) {
            self.emit(kind, payload);
        }
    }

    /// Command completion is not Agent task completion or a handoff signal.
    /// Never include error messages, which may contain page text or secret URLs.
    pub fn command_finished(&self, action: &'static str, error_kind: Option<&'static str>) {
        self.emit(
            "command.finished",
            json!({
                "action":action,
                "outcome":if error_kind.is_some() {"error"} else {"success"},
                "error_kind":error_kind,
            }),
        );
    }

    fn emit(&self, kind: &'static str, payload: Value) {
        if let Ok(mut emitter) = self.inner.lock() {
            emitter.emit(kind, payload);
        }
    }
}

impl Emitter {
    fn emit(&mut self, kind: &'static str, payload: Value) {
        self.seq += 1;
        let emitted_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(u64::MAX as u128) as u64;
        let envelope = json!({
            "token":self.token,
            "event":{
                "schema_version":1,
                "event_id":random_id(),
                "producer_id":self.producer_id,
                "operation_id":self.operation_id,
                "seq":self.seq,
                "emitted_at":emitted_at,
                "connection_ref":self.connection_ref,
                "session_id":self.session_id,
                "target_id":self.target_id,
                "run_id":self.run_id,
                "app_instance_id":self.app_instance_id,
                "kind":kind,
                "payload":payload,
            }
        });
        if let Ok(mut frame) = serde_json::to_vec(&envelope) {
            frame.push(b'\n');
            if frame.len() <= MAX_FRAME_BYTES {
                // Delivery errors are intentionally discarded. No logs, waits,
                // reconnect loops, acknowledgements or browser-command retries.
                let _ = send_frame(&self.endpoint, &frame);
            }
        }
    }
}

fn valid_connection(connection: &AppConnection) -> bool {
    connection.endpoint.is_absolute()
        && !connection.token.is_empty()
        && connection.token.len() <= 512
        && connection
            .app_instance_id
            .as_ref()
            .is_none_or(|id| !id.is_empty() && id.len() <= 128)
}

#[cfg(unix)]
fn discover_connection() -> Option<AppConnection> {
    let path = match env::var_os("LEXMOUNT_APP_CONNECTION_FILE") {
        Some(path) => PathBuf::from(path),
        None => dirs::home_dir()?.join(".local/state/lexmount/app-connection.json"),
    };
    read_connection_file(&path)
}

#[cfg(not(unix))]
fn discover_connection() -> Option<AppConnection> {
    None
}

#[cfg(unix)]
fn read_connection_file(path: &Path) -> Option<AppConnection> {
    use std::{
        fs::{self, OpenOptions},
        io::Read,
        os::unix::fs::{MetadataExt, OpenOptionsExt},
    };

    #[derive(serde::Deserialize)]
    struct ConnectionFile {
        schema_version: u64,
        endpoint: PathBuf,
        token: String,
        app_instance_id: String,
    }

    if !path.is_absolute() {
        return None;
    }
    // SAFETY: geteuid takes no arguments and has no caller preconditions.
    let uid = unsafe { libc::geteuid() };
    let parent = path.parent()?;
    let parent_metadata = fs::symlink_metadata(parent).ok()?;
    if !parent_metadata.is_dir()
        || parent_metadata.uid() != uid
        || parent_metadata.mode() & 0o7777 != 0o700
    {
        return None;
    }
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.uid() != uid || metadata.mode() & 0o7777 != 0o600 {
        return None;
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    let opened = file.metadata().ok()?;
    if !opened.is_file()
        || opened.uid() != uid
        || opened.mode() & 0o7777 != 0o600
        || opened.len() > MAX_FRAME_BYTES as u64
        || opened.dev() != metadata.dev()
        || opened.ino() != metadata.ino()
    {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_FRAME_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_FRAME_BYTES {
        return None;
    }
    let final_parent = fs::symlink_metadata(parent).ok()?;
    if final_parent.dev() != parent_metadata.dev()
        || final_parent.ino() != parent_metadata.ino()
        || final_parent.mode() & 0o7777 != 0o700
        || final_parent.uid() != uid
    {
        return None;
    }
    let config: ConnectionFile = serde_json::from_slice(&bytes).ok()?;
    if config.schema_version != 1 {
        return None;
    }
    let connection = AppConnection {
        endpoint: config.endpoint,
        token: config.token,
        app_instance_id: Some(config.app_instance_id),
    };
    valid_connection(&connection).then_some(connection)
}

fn random_id() -> String {
    format!("{:032x}", rand::random::<u128>())
}

/// Same derivation in the app: SHA-256(origin bytes + NUL + project ID bytes).
/// Path, query, userinfo, API key and raw project ID never enter an event.
pub fn connection_ref(api_base_url: &str, project_id: &str) -> Option<String> {
    let url = url::Url::parse(api_base_url).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return None;
    }
    let mut hash = Sha256::new();
    hash.update(url.origin().ascii_serialization().as_bytes());
    hash.update([0]);
    hash.update(project_id.as_bytes());
    Some(format!("lexmount:{:x}", hash.finalize()))
}

#[cfg(unix)]
fn send_frame(endpoint: &Path, frame: &[u8]) -> std::io::Result<()> {
    use socket2::{Domain, SockAddr, Socket, Type};
    use std::io::Write;

    let mut socket = Socket::new(Domain::UNIX, Type::STREAM, None)?;
    // Nonblocking from before connect: even a full accept backlog or a peer
    // that stops reading cannot hold up browser input. A partial frame is
    // discarded by the receiver when this connection closes.
    socket.set_nonblocking(true)?;
    socket.connect(&SockAddr::unix(endpoint)?)?;
    socket.write_all(frame)
}

#[cfg(not(unix))]
fn send_frame(_endpoint: &Path, _frame: &[u8]) -> std::io::Result<()> {
    // Windows named-pipe transport is a separate platform adapter. Until it is
    // implemented, events are disabled on Windows. Never fall back to TCP or a
    // potentially blocking CreateFile/WriteFile on a named pipe.
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "desktop event transport is not implemented on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_identity_is_namespaced_without_raw_sensitive_fields() {
        let identity = connection_ref(
            "https://user:secret@API.EXAMPLE.test/path?key=secret",
            "project-private",
        )
        .unwrap();
        assert_eq!(
            identity,
            connection_ref("https://api.example.test/other", "project-private").unwrap()
        );
        assert_ne!(
            identity,
            connection_ref("https://api.example.test", "other-project").unwrap()
        );
        assert!(!identity.contains("project-private"));
        assert!(!identity.contains("secret"));
        assert_eq!(identity.len(), "lexmount:".len() + 64);
        assert!(connection_ref("file:///tmp/private", "project").is_none());
        assert!(connection_ref("malformed", "project").is_none());
    }

    #[cfg(unix)]
    mod unix {
        use super::*;
        use std::{
            io::Read,
            os::unix::net::UnixListener,
            time::{Duration, Instant},
        };

        fn events_at(path: &Path) -> DesktopEvents {
            DesktopEvents::new(
                path.to_owned(),
                "test-ipc-token".into(),
                "namespace".into(),
                "session".into(),
                Some("run".into()),
            )
        }

        fn bounded_read(mut stream: std::os::unix::net::UnixStream) -> String {
            stream.set_nonblocking(true).unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut bytes = Vec::new();
            let mut buffer = [0u8; 1024];
            loop {
                match stream.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        bytes.extend_from_slice(&buffer[..count]);
                        assert!(bytes.len() <= MAX_FRAME_BYTES);
                    }
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("bounded event read: {error}"),
                }
                assert!(Instant::now() < deadline, "event fixture read timed out");
            }
            String::from_utf8(bytes).unwrap()
        }

        fn private_connection_file(directory: &Path, name: &str, value: Value) -> PathBuf {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700)).unwrap();
            let path = directory.join(name);
            std::fs::write(&path, value.to_string()).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            path
        }

        fn connection_value() -> Value {
            json!({"schema_version":1,"endpoint":"/tmp/lexmount-events.sock",
                   "token":"private-ipc-token","app_instance_id":"app-instance"})
        }

        #[test]
        fn discovery_requires_private_owned_regular_file_and_private_parent() {
            use std::os::unix::fs::{PermissionsExt, symlink};
            let directory = tempfile::tempdir().unwrap();
            let path = private_connection_file(
                directory.path(),
                "app-connection.json",
                connection_value(),
            );
            let connection = read_connection_file(&path).unwrap();
            assert_eq!(connection.token, "private-ipc-token");
            assert_eq!(connection.app_instance_id.as_deref(), Some("app-instance"));
            for mode in [0o644, 0o640, 0o400] {
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
                assert!(read_connection_file(&path).is_none());
            }
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o755))
                .unwrap();
            assert!(read_connection_file(&path).is_none());
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
            let link = directory.path().join("linked.json");
            symlink(&path, &link).unwrap();
            assert!(read_connection_file(&link).is_none());
            let outer = tempfile::tempdir().unwrap();
            let linked_parent = outer.path().join("linked-parent");
            symlink(directory.path(), &linked_parent).unwrap();
            assert!(read_connection_file(&linked_parent.join("app-connection.json")).is_none());
            assert!(read_connection_file(&directory.path().join("missing.json")).is_none());
        }

        #[test]
        fn discovery_rejects_malformed_oversize_and_unknown_schema_without_delivery() {
            let directory = tempfile::tempdir().unwrap();
            let mut values = Vec::new();
            let mut wrong_schema = connection_value();
            wrong_schema["schema_version"] = json!(2);
            values.push(wrong_schema);
            let mut relative = connection_value();
            relative["endpoint"] = json!("relative.sock");
            values.push(relative);
            let mut missing_token = connection_value();
            missing_token["token"] = json!("");
            values.push(missing_token);
            let mut missing_instance = connection_value();
            missing_instance["app_instance_id"] = json!("");
            values.push(missing_instance);
            let mut oversize = connection_value();
            oversize["token"] = json!("x".repeat(MAX_FRAME_BYTES));
            values.push(oversize);
            values.push(json!({}));
            for value in values {
                let path = private_connection_file(directory.path(), "app-connection.json", value);
                assert!(read_connection_file(&path).is_none());
            }
            let path = private_connection_file(
                directory.path(),
                "app-connection.json",
                connection_value(),
            );
            std::fs::write(&path, "{invalid").unwrap();
            assert!(read_connection_file(&path).is_none());
        }

        #[test]
        fn valid_ndjson_frames_preserve_operation_identity_and_sequence() {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("events.sock");
            let listener = UnixListener::bind(&path).unwrap();
            let events = events_at(&path);
            events.command_started("click");
            events.target_resolved("actual-target");
            events.command_finished("click", Some("cdp_error"));
            let mut frames = Vec::new();
            for _ in 0..3 {
                let (stream, _) = listener.accept().unwrap();
                let frame = bounded_read(stream);
                assert!(frame.ends_with('\n'));
                assert_eq!(frame.lines().count(), 1);
                let value: Value = serde_json::from_str(&frame).unwrap();
                assert_eq!(value["token"], "test-ipc-token");
                assert!(value["event"].get("token").is_none());
                frames.push(value["event"].clone());
            }
            assert_eq!(frames[0]["target_id"], Value::Null);
            assert_eq!(frames[1]["target_id"], "actual-target");
            assert_eq!(frames[2]["payload"]["error_kind"], "cdp_error");
            for (index, frame) in frames.iter().enumerate() {
                assert_eq!(frame["schema_version"], 1);
                assert_eq!(frame["seq"], index + 1);
                assert_eq!(frame["operation_id"], frames[0]["operation_id"]);
                assert_eq!(frame["producer_id"], frames[0]["producer_id"]);
                assert_eq!(frame["session_id"], "session");
                assert_eq!(frame["run_id"], "run");
                assert!(frame["emitted_at"].as_u64().unwrap() > 0);
                assert_eq!(frame["event_id"].as_str().unwrap().len(), 32);
            }
            assert_ne!(frames[0]["event_id"], frames[1]["event_id"]);
        }

        #[test]
        fn closed_or_nonreading_collector_never_waits_for_delivery() {
            let directory = tempfile::tempdir().unwrap();
            let missing = events_at(&directory.path().join("missing.sock"));
            let started = Instant::now();
            missing.command_started("click");
            assert!(started.elapsed() < Duration::from_millis(250));

            let path = directory.path().join("unread.sock");
            let _listener = UnixListener::bind(&path).unwrap();
            let events = events_at(&path);
            let started = Instant::now();
            // Fill the accept backlog. Connect must remain nonblocking after
            // the collector stops accepting, not just after it stops reading.
            for _ in 0..256 {
                events.command_started("click");
            }
            assert!(started.elapsed() < Duration::from_secs(1));
        }

        #[test]
        fn oversize_frames_are_dropped_without_a_connection() {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("bounded.sock");
            let listener = UnixListener::bind(&path).unwrap();
            listener.set_nonblocking(true).unwrap();
            events_at(&path).emit("fixture", json!({"oversize":"x".repeat(MAX_FRAME_BYTES)}));
            assert!(
                matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
            );
        }
    }
}
