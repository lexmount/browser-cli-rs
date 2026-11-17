#[allow(dead_code)]
mod support;
use httpmock::prelude::*;
use serde_json::json;
use std::{
    io::{BufRead, BufReader, Read},
    process::Stdio,
};

fn login_fixture(no_open: bool) {
    let server = MockServer::start();
    let exchange = server.mock(|when, then| {
        when.method(POST).path("/api/connect/codex/exchange");
        then.status(200).json_body(json!({"project_id":"fixture", "api_key":"fixture-secret", "api_base_url":"https://api.example.test"}));
    });
    let dir = tempfile::tempdir().unwrap();
    let credentials = dir.path().join("credentials.json");
    let mut cmd = support::command(
        &server.base_url(),
        dir.path(),
        &[
            "auth",
            "login",
            "--timeout-seconds",
            "5",
            "--connect-base-url",
            &server.base_url(),
            "--credentials-file",
            credentials.to_str().unwrap(),
        ],
    );
    if no_open {
        cmd.arg("--no-open");
    } else {
        cmd.arg("--browser")
            .env("PATH", dir.path())
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("BROWSER");
    }
    cmd.stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    let mut reader = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert!(line.contains("Open this URL"), "{line}");
    line.clear();
    reader.read_line(&mut line).unwrap();
    let url = url::Url::parse(line.trim()).unwrap();
    let params: std::collections::HashMap<_, _> = url.query_pairs().collect();
    let mut callback = url::Url::parse(&params["redirect_uri"]).unwrap();
    let http = reqwest::blocking::Client::builder()
        .no_proxy()
        .build()
        .unwrap();
    assert_eq!(http.get(callback.clone()).send().unwrap().status(), 400);
    callback
        .query_pairs_mut()
        .append_pair("state", &params["state"])
        .append_pair("code", "test-code");
    reqwest::blocking::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(callback)
        .send()
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let mut remaining_stderr = String::new();
    reader.read_to_string(&mut remaining_stderr).unwrap();
    if !no_open {
        assert!(
            remaining_stderr.contains("Could not open a local browser"),
            "{remaining_stderr}"
        );
    }
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("fixture-secret"));
    assert!(credentials.exists());
    exchange.assert();
}

#[test]
fn manual_login_prints_url_and_completes_without_desktop() {
    login_fixture(true);
}
#[cfg(target_os = "linux")]
#[test]
fn missing_desktop_opener_falls_back_to_manual_login() {
    login_fixture(false);
}
