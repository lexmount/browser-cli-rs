#[allow(dead_code)]
mod support;
use httpmock::prelude::*;
use serde_json::json;

fn fixture(error: Option<&str>, auto: bool, mismatch: bool) {
    fixture_url(error, auto, mismatch, "https://api.example.test");
}
fn fixture_url(error: Option<&str>, auto: bool, mismatch: bool, api_url: &str) {
    let server = MockServer::start();
    let create = server.mock(|when, then| {
        when.method(POST).path("/api/connect/device/authorize");
        then.status(200).json_body(json!({"device_code":"private-device-secret", "user_code":"ABCD-EFGH", "verification_uri":"/connect/device", "expires_in":3,"interval":1}));
    });
    let poll = server.mock(|when, then| {
        when.method(POST).path("/api/connect/device/token").json_body(json!({"device_code":"private-device-secret"}));
        if let Some(error) = error {
            then.status(400).json_body(json!({"error":error}));
        } else {
            then.status(200).json_body(json!({"project_id":"fixture", "api_key":"fixture-api-secret", "api_base_url":api_url}));
        }
    });
    let dir = tempfile::tempdir().unwrap();
    let credential = dir.path().join("device.json");
    let mut command = support::command(
        &server.base_url(),
        dir.path(),
        &[
            "auth",
            "login",
            "--connect-base-url",
            &server.base_url(),
            "--credentials-file",
            credential.to_str().unwrap(),
        ],
    );
    command.env_remove("CI");
    if auto {
        command.env("SSH_CONNECTION", "fixture");
    } else {
        command.arg("--device");
    }
    if mismatch {
        command.args(["--project-id", "another-project"]);
    }
    let output = command.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("ABCD-EFGH"));
    for secret in ["private-device-secret", "fixture-api-secret"] {
        assert!(!stdout.contains(secret));
        assert!(!stderr.contains(secret));
    }
    assert_eq!(
        output.status.success(),
        error.is_none() && !mismatch && api_url.starts_with("https://"),
        "{stdout} {stderr}"
    );
    assert_eq!(
        credential.exists(),
        error.is_none() && !mismatch && api_url.starts_with("https://")
    );
    #[cfg(unix)]
    if credential.exists() {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(credential).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    create.assert();
    assert!(poll.hits() >= 1);
}
#[test]
fn device_success_saves_private_credentials() {
    fixture(None, false, false);
}
#[test]
fn ssh_selects_device_flow() {
    fixture(None, true, false);
}
#[test]
fn denial_does_not_save_credentials() {
    fixture(Some("access_denied"), false, false);
}
#[test]
fn expiry_does_not_save_credentials() {
    fixture(Some("expired_token"), false, false);
}
#[test]
fn pending_stops_at_deadline() {
    fixture(Some("authorization_pending"), false, false);
}
#[test]
fn slow_down_stops_at_deadline() {
    fixture(Some("slow_down"), false, false);
}
#[test]
fn project_mismatch_does_not_save_credentials() {
    fixture(None, false, true);
}

#[test]
fn plaintext_api_origin_is_rejected() {
    fixture_url(None, false, false, "http://api.example.test");
}
