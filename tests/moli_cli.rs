#![cfg(unix)]
mod support;

use serde_json::Value;
use std::{path::Path, process::Output};

fn cli(directory: &Path, arguments: &[&str]) -> Output {
    let mut args = vec!["--provider", "moli"];
    args.extend_from_slice(arguments);
    support::command("http://127.0.0.1:1", directory, &args)
        .env("LEXMOUNT_MOLI_STATE_DIR", directory.join("instances"))
        // Deliberately invalid forced cloud configuration must never be consulted.
        .env("LEXMOUNT_AUTH_SOURCE", "desktop")
        .env(
            "LEXMOUNT_DESKTOP_EXPECTED_PROJECT_ID",
            "mismatched-cloud-project",
        )
        .env(
            "LEXMOUNT_DESKTOP_EXPECTED_API_BASE_URL",
            "invalid-cloud-origin",
        )
        .output()
        .unwrap()
}

#[test]
fn local_listing_needs_no_account_and_has_compatible_pagination() {
    let temp = tempfile::tempdir().unwrap();
    let data = support::data(cli(
        temp.path(),
        &[
            "session",
            "list",
            "--status",
            "active",
            "--page",
            "1",
            "--page-size",
            "100",
        ],
    ));
    assert_eq!(data["provider"], "moli");
    assert_eq!(data["sessions"], serde_json::json!([]));
    assert_eq!(data["pagination"]["currentPage"], 1);
    assert_eq!(data["pagination"]["totalPages"], 1);
    assert_eq!(data["pagination"]["totalCount"], 0);
}

#[test]
fn unsupported_cloud_options_fail_before_any_moli_state_or_auth_access() {
    let temp = tempfile::tempdir().unwrap();
    for args in [
        vec!["context", "list"],
        vec!["auth", "logout"],
        vec!["tunnel", "open"],
        vec!["session", "create", "--official-proxy"],
        vec!["session", "create", "--downloads"],
        vec!["session", "create", "--context-mode", "read_write"],
        vec!["session", "create", "--browser-mode", "normal"],
    ] {
        let output = cli(temp.path(), &args);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["error"], "configuration_error");
        assert!(!temp.path().join("instances").exists());
    }
}

#[test]
fn malformed_or_foreign_instance_ids_do_not_turn_into_connection_addresses() {
    let temp = tempfile::tempdir().unwrap();
    for id in [
        "../escape",
        "session_cloud",
        "ws://127.0.0.1:9222/devtools/browser/test",
    ] {
        let output = cli(temp.path(), &["session", "get", "--session-id", id]);
        assert!(!output.status.success());
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["error"], "configuration_error");
    }
}
