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
