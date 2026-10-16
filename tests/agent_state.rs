//! Explicit task reports use the local event channel, never a cloud task API.
#![cfg(unix)]

mod support;

use serde_json::Value;
use std::{
    io::{ErrorKind, Read},
    os::unix::net::{UnixListener, UnixStream},
    path::Path,
    thread,
    time::{Duration, Instant},
};

fn state_command(directory: &Path, endpoint: &Path) -> std::process::Command {
    let mut command = support::command(
        "http://127.0.0.1:1",
        directory,
        &[
            "agent",
            "state",
            "--session-id",
            "browser",
            "--target-id",
            "wanted",
            "--run-id",
            "explicit-run",
            "--state",
            "needs_human",
            "--state-version",
            "2",
            "--handoff-id",
            "handoff-1",
        ],
    );
    command
        .env_remove("LEXMOUNT_APP_EVENTS")
        .env("LEXMOUNT_APP_EVENT_ENDPOINT", endpoint)
        .env("LEXMOUNT_APP_EVENT_TOKEN", "independent-ipc-token")
        .env("LEXMOUNT_AGENT_RUN_ID", "must-not-replace-explicit-run");
    command
}

fn bounded_read(mut stream: UnixStream) -> Value {
    stream.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut bytes = Vec::new();
    let mut buffer = [0; 1024];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                bytes.extend_from_slice(&buffer[..count]);
                assert!(bytes.len() <= 16 * 1024);
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(1))
            }
            Err(e) => panic!("bounded state event read: {e}"),
        }
        assert!(Instant::now() < deadline, "state event read timed out");
    }
    assert!(bytes.ends_with(b"\n"));
    assert_eq!(bytes.iter().filter(|b| **b == b'\n').count(), 1);
    serde_json::from_slice(&bytes).unwrap()
}

#[test]
fn cli_reports_explicit_handoff_state_without_calling_unreachable_cloud_api() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.sock");
    let listener = UnixListener::bind(&path).unwrap();
    listener.set_nonblocking(true).unwrap();
    let result = support::data(state_command(directory.path(), &path).output().unwrap());
    assert_eq!(result["attempted"], true);
    assert_eq!(result["delivery"], "best_effort");
    assert_eq!(result["server_persisted"], false);
    let (stream, _) = listener.accept().unwrap();
    let frame = bounded_read(stream);
    assert_eq!(frame["token"], "independent-ipc-token");
    let event = &frame["event"];
    assert_eq!(event["kind"], "task.state");
    assert_eq!(event["seq"], 1);
    assert_eq!(event["run_id"], "explicit-run");
    assert_eq!(event["session_id"], "browser");
    assert_eq!(event["target_id"], "wanted");
    assert_eq!(event["payload"]["state"], "needs_human");
    assert_eq!(event["payload"]["state_version"], 2);
    assert_eq!(event["payload"]["handoff_id"], "handoff-1");
    assert!(matches!(listener.accept(), Err(e) if e.kind() == ErrorKind::WouldBlock));
    let text = frame.to_string();
    assert!(!text.contains("local-test-key"));
    assert!(!text.contains("local-test-project"));
}

#[test]
fn stopped_or_disabled_app_does_not_turn_a_task_report_into_task_failure() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("missing.sock");
    let result = support::data(state_command(directory.path(), &path).output().unwrap());
    assert_eq!(result["attempted"], true);
    assert_eq!(result["delivery"], "best_effort");
    let result = support::data(
        state_command(directory.path(), &path)
            .env("LEXMOUNT_APP_EVENTS", "off")
            .output()
            .unwrap(),
    );
    assert_eq!(result["attempted"], false);
    assert_eq!(result["server_persisted"], false);
}
