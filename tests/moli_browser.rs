#![cfg(unix)]
//! Explicit opt-in: operates only on an instance created by this test.
mod support;
use serde_json::{Value, json};
use std::{
    io::Read,
    net::{SocketAddr, TcpStream},
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};

struct Fixture {
    directory: tempfile::TempDir,
    binary: PathBuf,
    id: String,
}
impl Fixture {
    fn start() -> (Self, Value) {
        let binary = PathBuf::from(
            std::env::var_os("BROWSER_CLI_TEST_MOLI").expect("set BROWSER_CLI_TEST_MOLI"),
        );
        assert!(binary.is_absolute());
        let mut fixture = Self {
            directory: tempfile::tempdir().unwrap(),
            binary,
            id: String::new(),
        };
        let created = fixture.run(&["session", "create", "--timeout-seconds", "12"]);
        fixture.id = created["session_id"].as_str().unwrap().to_owned();
        (fixture, created)
    }
    fn command(&self, arguments: &[&str]) -> Command {
        let mut args = vec!["--provider", "moli"];
        args.extend_from_slice(arguments);
        let mut command = support::command("http://127.0.0.1:1", self.directory.path(), &args);
        command
            .env("LEXMOUNT_MOLI_BINARY", &self.binary)
            .env(
                "LEXMOUNT_MOLI_STATE_DIR",
                self.directory.path().join("instances"),
            )
            .env("LEXMOUNT_AUTH_SOURCE", "desktop")
            .env("LEXMOUNT_DESKTOP_EXPECTED_PROJECT_ID", "not-cloud")
            .env("LEXMOUNT_DESKTOP_EXPECTED_API_BASE_URL", "not-cloud");
        command
    }
    fn run(&self, args: &[&str]) -> Value {
        support::data(bounded_output(&mut self.command(args), Duration::from_secs(20)).unwrap())
    }
    fn action(&self, target: &str, verb: &str, args: &[&str]) -> Value {
        let mut command = vec![
            "action",
            "--target-id",
            target,
            verb,
            "--session-id",
            &self.id,
        ];
        command.extend_from_slice(args);
        self.run(&command)
    }
    fn raw(&self, owner: &str, method: &str, parameters: Value) -> Value {
        self.action(
            owner,
            "raw",
            &["--method", method, "--params-json", &parameters.to_string()],
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !self.id.is_empty() {
            let _ = bounded_output(
                &mut self.command(&["session", "close", "--session-id", &self.id]),
                Duration::from_secs(5),
            );
        }
    }
}

fn bounded_output(
    command: &mut Command,
    timeout: Duration,
) -> std::io::Result<std::process::Output> {
    let mut child = command.spawn()?;
    let deadline = Instant::now() + timeout;
    while child.try_wait()?.is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "owned Moli CLI command timed out",
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    child.wait_with_output()
}

#[test]
#[ignore = "requires BROWSER_CLI_TEST_MOLI pointing to the verified official Moli executable; creates and closes its own process"]
fn real_moli_owned_lifecycle_actions_and_local_task_events_without_cloud_auth() {
    let (fixture, created) = Fixture::start();
    assert!(fixture.id.starts_with("moli_"));
    assert_eq!(created["provider"], "moli");
    assert_eq!(created["status"], "active");
    assert_eq!(
        created["connection_ref"],
        lexmount_browser::moli::connection_ref(&fixture.id).unwrap()
    );
    let get = fixture.run(&["session", "get", "--session-id", &fixture.id]);
    assert_eq!(get, created);
    let list = fixture.run(&["session", "list", "--status", "active"]);
    assert_eq!(list["sessions"].as_array().unwrap().len(), 1);
    let targets = fixture.run(&["session", "targets", "--session-id", &fixture.id]);
    let target = targets
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["type"] == "page")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(fixture.action(&target, "eval", &["--expression", "document.body.innerHTML='<button id=counter style=\"margin:80px\" onclick=\"this.textContent=String(Number(this.textContent)+1)\">0</button><input id=name style=\"margin:20px\">';true"]), true);
    let point = fixture.action(
        &target,
        "click",
        &["--selector", "#counter", "--report-point"],
    );
    assert_eq!(
        fixture.action(
            &target,
            "eval",
            &[
                "--expression",
                "document.querySelector('#counter').textContent"
            ]
        ),
        "1"
    );
    let origin = serde_json::to_string(&point).unwrap();
    let moved = fixture.action(
        &target,
        "move",
        &[
            "--selector",
            "#name",
            "--from-json",
            &origin,
            "--duration-ms",
            "200",
        ],
    );
    assert!(moved["steps"].as_u64().unwrap() >= 2);
    assert_eq!(
        fixture.action(
            &target,
            "fill",
            &["--selector", "#name", "--value", "中文Moli"]
        ),
        true
    );
    assert_eq!(
        fixture.action(
            &target,
            "eval",
            &["--expression", "document.querySelector('#name').value"]
        ),
        "中文Moli"
    );
    let image = fixture.directory.path().join("owned.png");
    fixture.action(&target, "screenshot", &["--path", image.to_str().unwrap()]);
    assert!(
        std::fs::read(&image)
            .unwrap()
            .starts_with(b"\x89PNG\r\n\x1a\n")
    );

    // Validate a real UDS frame, rather than treating attempted:true as delivery proof.
    let socket = fixture.directory.path().join("e.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut state_command = fixture.command(&[
        "agent",
        "state",
        "--session-id",
        &fixture.id,
        "--target-id",
        &target,
        "--run-id",
        "moli-test-run",
        "--state",
        "needs_human",
        "--state-version",
        "1",
        "--handoff-id",
        "moli-test-handoff",
    ]);
    state_command
        .env_remove("LEXMOUNT_APP_EVENTS")
        .env("LEXMOUNT_APP_EVENT_ENDPOINT", &socket)
        .env(
            "LEXMOUNT_APP_EVENT_TOKEN",
            "moli-test-ipc-token-independent",
        );
    let output = bounded_output(&mut state_command, Duration::from_secs(20)).unwrap();
    let state = support::data(output);
    assert_eq!(state["attempted"], true);
    assert_eq!(state["server_persisted"], false);
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(error) => panic!("owned event socket failed: {error}"),
        }
    };
    stream.set_nonblocking(true).unwrap();
    let mut bytes = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => bytes.extend_from_slice(&buffer[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5))
            }
            Err(e) => panic!("owned frame read failed: {e}"),
        }
    }
    let frame: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(frame["event"]["kind"], "task.state");
    assert_eq!(frame["event"]["connection_ref"], created["connection_ref"]);
    assert_eq!(frame["event"]["session_id"], fixture.id);
    assert_eq!(frame["event"]["target_id"], target);
    assert_eq!(frame["event"]["payload"]["state"], "needs_human");
    assert!(!String::from_utf8(bytes).unwrap().contains("not-cloud"));

    assert_eq!(
        fixture.run(&["session", "close", "--session-id", &fixture.id])["closed"],
        true
    );
    assert_eq!(
        fixture.run(&["session", "get", "--session-id", &fixture.id])["status"],
        "closed"
    );
    assert_eq!(
        fixture.run(&["session", "list", "--status", "active"])["sessions"],
        json!([])
    );
}

// A bounded, direct page frontend for the test-owned loopback endpoint only.
// It deliberately has no account, proxy override, remote executor or native UI access.
struct ProtocolFrontend {
    socket: tungstenite::WebSocket<TcpStream>,
    next_id: u64,
}
impl ProtocolFrontend {
    fn connect(url: &str, target: Option<&str>, owned_origin: &str) -> Self {
        let endpoint = url::Url::parse(url).unwrap();
        let origin = url::Url::parse(owned_origin).unwrap();
        assert_eq!(endpoint.scheme(), "ws");
        assert_eq!(endpoint.host_str(), Some("127.0.0.1"));
        assert_eq!(endpoint.port(), origin.port());
        if let Some(target) = target {
            assert_eq!(endpoint.path(), format!("/devtools/page/{target}"));
        } else {
            assert!(
                endpoint.path().starts_with("/devtools/browser/")
                    && endpoint.path().len() > "/devtools/browser/".len()
            );
        }
        assert!(
            endpoint.username().is_empty()
                && endpoint.password().is_none()
                && endpoint.query().is_none()
        );
        let address = SocketAddr::from(([127, 0, 0, 1], endpoint.port().unwrap()));
        let stream = TcpStream::connect_timeout(&address, Duration::from_secs(3)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let config = tungstenite::protocol::WebSocketConfig::default()
            .max_message_size(Some(1024 * 1024))
            .max_frame_size(Some(1024 * 1024));
        let (socket, _) =
            tungstenite::client::client_with_config(url, stream, Some(config)).unwrap();
        Self { socket, next_id: 1 }
    }
    fn command(&mut self, method: &str, parameters: Value) -> Value {
        self.command_on(None, method, parameters)
    }
    fn command_on(&mut self, session_id: Option<&str>, method: &str, parameters: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let mut request = json!({"id":id,"method":method,"params":parameters});
        if let Some(session_id) = session_id {
            request["sessionId"] = session_id.into();
        }
        self.socket
            .send(tungstenite::Message::Text(request.to_string().into()))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        for _ in 0..128 {
            self.socket
                .get_mut()
                .set_read_timeout(Some(
                    deadline
                        .checked_duration_since(Instant::now())
                        .filter(|d| !d.is_zero())
                        .expect("owned page command timed out"),
                ))
                .unwrap();
            let message = self.socket.read().unwrap();
            if !message.is_text() {
                continue;
            }
            let message: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
            if message["id"] == id {
                assert!(
                    message.get("error").is_none(),
                    "owned page method rejected: {method}"
                );
                return message["result"].clone();
            }
        }
        panic!("owned page emitted too many events before ACK");
    }
    fn marker(&mut self, session_id: Option<&str>) -> Value {
        self.command_on(
            session_id,
            "Runtime.evaluate",
            json!({"expression":"globalThis.__ownedMarker","returnByValue":true}),
        )["result"]["value"]
            .clone()
    }
}
impl Drop for ProtocolFrontend {
    fn drop(&mut self) {
        let _ = self.socket.close(None);
    }
}

#[test]
#[ignore = "requires verified BROWSER_CLI_TEST_MOLI; creates three owned tabs and closes only its own instance"]
fn real_moli_exact_tabs_and_frontend_disconnect_preserve_other_tabs_and_process() {
    let (fixture, created) = Fixture::start();
    let first = fixture.run(&["session", "targets", "--session-id", &fixture.id]);
    let seed = first
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["type"] == "page")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut targets = vec![seed.clone()];
    for _ in 0..2 {
        let result = fixture.raw(&seed, "Target.createTarget", json!({"url":"about:blank"}));
        targets.push(result["targetId"].as_str().unwrap().to_owned());
    }
    assert_eq!(
        targets
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        3
    );
    let markers = ["owned-alpha", "owned-beta", "owned-gamma"];
    for (target, marker) in targets.iter().zip(markers) {
        fixture.action(
            target,
            "wait-selector",
            &["--selector", "body", "--timeout-ms", "5000"],
        );
        let expression = format!(
            "globalThis.__ownedMarker={};globalThis.__ownedCount=0;document.title=globalThis.__ownedMarker;document.body.innerHTML='<button id=counter style=\"margin:80px\" onclick=\"globalThis.__ownedCount++\">increment</button>';true",
            serde_json::to_string(marker).unwrap()
        );
        assert_eq!(
            fixture.action(target, "eval", &["--expression", &expression]),
            true
        );
    }
    let listed = fixture.run(&["session", "targets", "--session-id", &fixture.id]);
    assert_eq!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["type"] == "page")
            .count(),
        3
    );
    let mut counts = [0, 0, 0];
    for selected in [2, 0, 1, 0] {
        fixture.raw(
            &seed,
            "Target.activateTarget",
            json!({"targetId":targets[selected]}),
        );
        assert_eq!(
            fixture.action(&targets[selected], "click", &["--selector", "#counter"]),
            true
        );
        counts[selected] += 1;
        for (index, target) in targets.iter().enumerate() {
            assert_eq!(
                fixture.action(
                    target,
                    "eval",
                    &[
                        "--expression",
                        "({marker:globalThis.__ownedMarker,count:globalThis.__ownedCount})"
                    ]
                ),
                json!({"marker":markers[index],"count":counts[index]})
            );
        }
    }

    let browser_ws = created["ws"].as_str().unwrap();
    let page_ws = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == seed)
        .unwrap()["webSocketDebuggerUrl"]
        .as_str()
        .unwrap();
    let origin = created["http_endpoint"].as_str().unwrap();
    let mut browser = ProtocolFrontend::connect(browser_ws, None, origin);
    let attachment = browser.command(
        "Target.attachToTarget",
        json!({"targetId":seed,"flatten":true}),
    );
    let browser_session = attachment["sessionId"].as_str().unwrap().to_owned();
    browser.command_on(Some(&browser_session), "Runtime.enable", json!({}));
    let mut page = ProtocolFrontend::connect(page_ws, Some(&seed), origin);
    page.command("Runtime.enable", json!({}));
    assert_eq!(page.marker(None), markers[0]);
    assert_eq!(browser.marker(Some(&browser_session)), markers[0]);
    let attachment = browser.command(
        "Target.attachToTarget",
        json!({"targetId":targets[2],"flatten":true}),
    );
    browser.command(
        "Target.detachFromTarget",
        json!({"sessionId":attachment["sessionId"].as_str().unwrap()}),
    );
    assert_eq!(page.marker(None), markers[0]);
    assert_eq!(
        fixture.action(
            &targets[2],
            "eval",
            &["--expression", "globalThis.__ownedMarker"]
        ),
        markers[2]
    );
    drop(browser);
    assert_eq!(
        page.marker(None),
        markers[0],
        "browser frontend detach must not retire page frontend"
    );
    assert_eq!(
        fixture.run(&["session", "get", "--session-id", &fixture.id])["status"],
        "active"
    );
    let mut browser = ProtocolFrontend::connect(browser_ws, None, origin);
    let attachment = browser.command(
        "Target.attachToTarget",
        json!({"targetId":seed,"flatten":true}),
    );
    let browser_session = attachment["sessionId"].as_str().unwrap().to_owned();
    browser.command_on(Some(&browser_session), "Runtime.enable", json!({}));
    drop(page);
    assert_eq!(
        browser.marker(Some(&browser_session)),
        markers[0],
        "page frontend detach must not retire browser frontend"
    );
    drop(browser);

    assert_eq!(
        fixture.raw(&seed, "Target.closeTarget", json!({"targetId":targets[1]}))["success"],
        true
    );
    let remaining = fixture.run(&["session", "targets", "--session-id", &fixture.id]);
    assert!(
        !remaining
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == targets[1])
    );
    assert_eq!(
        remaining
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["type"] == "page")
            .count(),
        2
    );
    let closed = bounded_output(
        &mut fixture.command(&[
            "action",
            "--target-id",
            &targets[1],
            "eval",
            "--session-id",
            &fixture.id,
            "--expression",
            "globalThis.__ownedMarker",
        ]),
        Duration::from_secs(20),
    )
    .unwrap();
    assert!(!closed.status.success());
    assert!(
        closed.stdout.is_empty(),
        "closed Target must not fall back to another page"
    );
    let closed_error: Value = serde_json::from_slice(&closed.stderr).unwrap();
    assert_eq!(closed_error["error"], "not_found");
    for index in [0, 2] {
        assert_eq!(
            fixture.action(
                &targets[index],
                "eval",
                &[
                    "--expression",
                    "({marker:globalThis.__ownedMarker,count:globalThis.__ownedCount})"
                ]
            ),
            json!({"marker":markers[index],"count":counts[index]})
        );
    }
    assert_eq!(
        fixture.run(&["session", "get", "--session-id", &fixture.id])["status"],
        "active",
        "disconnecting every viewer must leave the owned process alive"
    );
    assert_eq!(
        fixture.run(&["session", "close", "--session-id", &fixture.id])["closed"],
        true
    );
}

// Run the public SDK in a separate test worker, so its Moli children can be
// reaped with waitpid without mutating this test process's global environment.
#[test]
#[ignore = "requires a Browser.close-capable dev BROWSER_CLI_TEST_MOLI and BROWSER_CLI_TEST_MOLI_BROWSER_CLOSE=1; creates only owned instances"]
fn real_moli_browser_close_naturally_exits_only_its_owned_process() {
    run_browser_close_test(
        "real_moli_browser_close_naturally_exits_only_its_owned_process",
        browser_close_process_worker,
        "MOLI_BROWSER_CLOSE_OS_PROOF exit_zero=2 listener_closed=2 viewer_disconnect_survived=true sentinel_survived=true",
    );
}

#[test]
#[ignore = "requires a Browser.close-capable dev BROWSER_CLI_TEST_MOLI and capability flag; verifies production close on owned processes"]
fn real_moli_sdk_close_prefers_verified_protocol_and_naturally_exits() {
    run_browser_close_test(
        "real_moli_sdk_close_prefers_verified_protocol_and_naturally_exits",
        sdk_close_process_worker,
        "MOLI_SDK_CLOSE_OS_PROOF exit_zero=2 listener_closed=2 viewer_disconnect_survived=true sentinel_survived=true",
    );
}

fn run_browser_close_test(name: &str, worker: fn(), proof: &str) {
    assert_eq!(
        std::env::var("BROWSER_CLI_TEST_MOLI_BROWSER_CLOSE").as_deref(),
        Ok("1"),
        "explicit dev Browser.close capability opt-in is required"
    );
    let binary = PathBuf::from(
        std::env::var_os("BROWSER_CLI_TEST_MOLI").expect("set dev BROWSER_CLI_TEST_MOLI"),
    );
    assert!(binary.is_absolute() && binary.is_file());
    if std::env::var("MOLI_BROWSER_CLOSE_TEST_WORKER").as_deref() == Ok(name) {
        worker();
        return;
    }

    let private = tempfile::tempdir().unwrap();
    let registry = private.path().join("instances");
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", name, "--ignored", "--nocapture"])
        .env("MOLI_BROWSER_CLOSE_TEST_WORKER", name)
        .env("LEXMOUNT_MOLI_STATE_DIR", &registry)
        .env("LEXMOUNT_MOLI_BINARY", &binary)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // Proxy settings remain inherited. Existing support::command and
    // support::isolated_test clear them, so neither helper is used here.
    let output = bounded_output(&mut command, Duration::from_secs(90));

    // The hard timeout can skip worker destructors. Recover only records in
    // this newly created private registry; cleanup never counts as test proof.
    let mut cleanup_ok = true;
    if registry.is_dir() {
        for entry in std::fs::read_dir(&registry).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|v| v.to_str()) != Some("json") {
                continue;
            }
            let id = path.file_stem().and_then(|v| v.to_str()).unwrap();
            assert!(
                id.len() == 37
                    && id.starts_with("moli_")
                    && id[5..].bytes().all(|b| b.is_ascii_hexdigit())
            );
            let mut cleanup = Command::new(env!("CARGO_BIN_EXE_browser-cli"));
            cleanup
                .args(["--provider", "moli", "session", "close", "--session-id", id])
                .env("LEXMOUNT_MOLI_STATE_DIR", &registry)
                .env("LEXMOUNT_MOLI_BINARY", &binary)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            cleanup_ok &= bounded_output(&mut cleanup, Duration::from_secs(5))
                .is_ok_and(|o| o.status.success());
        }
    }
    let output = output.expect("owned worker exceeded its bound or failed to spawn");
    assert!(
        output.status.success(),
        "owned Browser.close worker failed; cleanup is not qualification"
    );
    assert!(cleanup_ok, "owned registry cleanup failed");
    assert!(String::from_utf8_lossy(&output.stdout).contains(proof));
    println!("{proof}");
}

struct BrowserCloseOwnedProcesses {
    moli: lexmount_browser::moli::Moli,
    registry: PathBuf,
    ids: Vec<String>,
}
impl BrowserCloseOwnedProcesses {
    fn create(&mut self) -> (Value, libc::pid_t) {
        let row = self.moli.create(Duration::from_secs(12)).unwrap();
        let id = row["session_id"].as_str().unwrap().to_owned();
        self.ids.push(id.clone());
        assert_eq!(self.moli.get(&id).unwrap()["status"], "active");
        let record: Value = serde_json::from_slice(
            &std::fs::read(self.registry.join(format!("{id}.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(record["closed"], false);
        assert!(
            record["process"]["start_identity"]
                .as_str()
                .is_some_and(|v| !v.is_empty())
        );
        let pid = libc::pid_t::try_from(record["process"]["pid"].as_u64().unwrap()).unwrap();
        assert!(pid > 1);
        (row, pid)
    }

    fn confirm_natural_exit(&self, row: &Value, pid: libc::pid_t) {
        let deadline = Instant::now() + Duration::from_secs(7);
        let mut status = 0;
        loop {
            // This PID is a direct child created by this worker's Moli SDK.
            let waited = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
            if waited == pid {
                break;
            }
            if waited < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            assert_eq!(waited, 0, "own direct child must remain waitable");
            assert!(
                Instant::now() < deadline,
                "Browser.close did not exit its own process in time"
            );
            std::thread::sleep(Duration::from_millis(15));
        }
        assert!(
            libc::WIFEXITED(status),
            "signal termination is not natural exit"
        );
        assert_eq!(
            libc::WEXITSTATUS(status),
            0,
            "shutdown must return process exit 0"
        );
        let origin = url::Url::parse(row["http_endpoint"].as_str().unwrap()).unwrap();
        assert_eq!(origin.host_str(), Some("127.0.0.1"));
        let address = SocketAddr::from(([127, 0, 0, 1], origin.port().unwrap()));
        assert!(
            TcpStream::connect_timeout(&address, Duration::from_millis(150)).is_err(),
            "owned listener must disappear"
        );
        let id = row["session_id"].as_str().unwrap();
        assert_eq!(
            self.moli.get(id).unwrap()["status"],
            "closed",
            "original registry kernel identity must no longer match"
        );
        assert_eq!(
            self.moli.close(id).unwrap()["already_inactive"],
            true,
            "post-proof registry cleanup must not signal"
        );
    }
}
impl Drop for BrowserCloseOwnedProcesses {
    fn drop(&mut self) {
        for id in &self.ids {
            let _ = self.moli.close(id);
            if let Ok(bytes) = std::fs::read(self.registry.join(format!("{id}.json")))
                && let Ok(record) = serde_json::from_slice::<Value>(&bytes)
                && let Some(pid) = record["process"]["pid"]
                    .as_u64()
                    .and_then(|p| libc::pid_t::try_from(p).ok())
            {
                let deadline = Instant::now() + Duration::from_secs(1);
                while unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) } == 0
                    && Instant::now() < deadline
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }
}

fn browser_close_process_worker() {
    use lexmount_browser::cdp::Cdp;
    let registry = PathBuf::from(std::env::var_os("LEXMOUNT_MOLI_STATE_DIR").unwrap());
    assert!(registry.is_absolute());
    let mut owned = BrowserCloseOwnedProcesses {
        moli: lexmount_browser::moli::Moli::from_env().unwrap(),
        registry,
        ids: Vec::new(),
    };
    assert_eq!(
        std::fs::read_dir(&owned.registry).unwrap().count(),
        0,
        "worker accepts only its fresh registry"
    );
    let (first, first_pid) = owned.create();
    let (sentinel, sentinel_pid) = owned.create();
    let first_ws = first["ws"].as_str().unwrap();
    let sentinel_ws = sentinel["ws"].as_str().unwrap();
    assert_ne!(first["http_endpoint"], sentinel["http_endpoint"]);
    let viewer = Cdp::connect(first_ws).unwrap();
    drop(viewer);
    assert_eq!(
        owned
            .moli
            .get(first["session_id"].as_str().unwrap())
            .unwrap()["status"],
        "active"
    );
    let mut sentinel_view = Cdp::connect(sentinel_ws).unwrap();
    assert_eq!(
        sentinel_view
            .command(
                "Runtime.evaluate",
                json!({"expression":"globalThis.__ownedSentinel=73","returnByValue":true})
            )
            .unwrap()["result"]["value"],
        73
    );
    let mut root = Cdp::connect(first_ws).unwrap();
    assert_eq!(
        root.command_root("Browser.close", json!({})).unwrap(),
        json!({}),
        "root ACK is acceptance, not exit proof"
    );
    drop(root);
    owned.confirm_natural_exit(&first, first_pid);
    assert_eq!(
        owned
            .moli
            .get(sentinel["session_id"].as_str().unwrap())
            .unwrap()["status"],
        "active"
    );
    assert_eq!(
        sentinel_view
            .command(
                "Runtime.evaluate",
                json!({"expression":"globalThis.__ownedSentinel","returnByValue":true})
            )
            .unwrap()["result"]["value"],
        73
    );
    assert_eq!(
        sentinel_view
            .command_root("Browser.close", json!({}))
            .unwrap(),
        json!({})
    );
    drop(sentinel_view);
    owned.confirm_natural_exit(&sentinel, sentinel_pid);
    println!(
        "MOLI_BROWSER_CLOSE_OS_PROOF exit_zero=2 listener_closed=2 viewer_disconnect_survived=true sentinel_survived=true"
    );
}

fn sdk_close_process_worker() {
    use lexmount_browser::cdp::Cdp;
    let registry = PathBuf::from(std::env::var_os("LEXMOUNT_MOLI_STATE_DIR").unwrap());
    let mut owned = BrowserCloseOwnedProcesses {
        moli: lexmount_browser::moli::Moli::from_env().unwrap(),
        registry,
        ids: Vec::new(),
    };
    assert_eq!(std::fs::read_dir(&owned.registry).unwrap().count(), 0);
    let (first, first_pid) = owned.create();
    let (sentinel, sentinel_pid) = owned.create();
    drop(Cdp::connect(first["ws"].as_str().unwrap()).unwrap());
    assert_eq!(
        owned
            .moli
            .get(first["session_id"].as_str().unwrap())
            .unwrap()["status"],
        "active"
    );
    let mut sentinel_view = Cdp::connect(sentinel["ws"].as_str().unwrap()).unwrap();
    assert_eq!(
        sentinel_view
            .command(
                "Runtime.evaluate",
                json!({"expression":"globalThis.__ownedSentinel=91","returnByValue":true})
            )
            .unwrap()["result"]["value"],
        91
    );
    let result = owned
        .moli
        .close(first["session_id"].as_str().unwrap())
        .unwrap();
    assert_eq!(result["closed"], true);
    assert_eq!(result["already_inactive"], false);
    owned.confirm_natural_exit(&first, first_pid);
    assert_eq!(
        sentinel_view
            .command(
                "Runtime.evaluate",
                json!({"expression":"globalThis.__ownedSentinel","returnByValue":true})
            )
            .unwrap()["result"]["value"],
        91
    );
    assert_eq!(
        owned
            .moli
            .close(sentinel["session_id"].as_str().unwrap())
            .unwrap()["closed"],
        true
    );
    drop(sentinel_view);
    owned.confirm_natural_exit(&sentinel, sentinel_pid);
    println!(
        "MOLI_SDK_CLOSE_OS_PROOF exit_zero=2 listener_closed=2 viewer_disconnect_survived=true sentinel_survived=true"
    );
}
