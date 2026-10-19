#![cfg(unix)]
//! Explicit opt-in: operates only on an instance created by this test.
mod support;
use serde_json::{Value, json};
use std::{
    io::Read,
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
        support::data(self.command(args).output().unwrap())
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
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !self.id.is_empty() {
            let _ = self
                .command(&["session", "close", "--session-id", &self.id])
                .output();
        }
    }
}

#[test]
#[ignore = "requires BROWSER_CLI_TEST_MOLI pointing to the verified official Moli executable; creates and closes its own process"]
fn real_moli_owned_lifecycle_actions_and_local_task_events_without_cloud_auth() {
    let binary = PathBuf::from(
        std::env::var_os("BROWSER_CLI_TEST_MOLI").expect("set BROWSER_CLI_TEST_MOLI"),
    );
    assert!(binary.is_absolute());
    let mut fixture = Fixture {
        directory: tempfile::tempdir().unwrap(),
        binary,
        id: String::new(),
    };
    let created = fixture.run(&["session", "create", "--timeout-seconds", "12"]);
    fixture.id = created["session_id"].as_str().unwrap().to_owned();
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
    let output = fixture
        .command(&[
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
        ])
        .env_remove("LEXMOUNT_APP_EVENTS")
        .env("LEXMOUNT_APP_EVENT_ENDPOINT", &socket)
        .env(
            "LEXMOUNT_APP_EVENT_TOKEN",
            "moli-test-ipc-token-independent",
        )
        .output()
        .unwrap();
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
