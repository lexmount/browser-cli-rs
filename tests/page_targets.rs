//! Deterministic protocol and CLI tests. Only local HTTP/WebSocket fixtures run.
mod support;

use httpmock::{Method::POST, MockServer};
use lexmount_browser::cdp::Cdp;
use serde_json::{Value, json};
use std::{
    fs,
    io::ErrorKind,
    net::TcpListener,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tungstenite::{Message, error::ProtocolError};

struct Peer {
    url: String,
    worker: JoinHandle<Vec<Value>>,
}

impl Peer {
    fn start(targets: Value, fail_attach: bool) -> Self {
        Self::with_exception(targets, fail_attach, None)
    }

    fn with_exception(targets: Value, fail_attach: bool, exception: Option<Value>) -> Self {
        Self::with_responses(targets, fail_attach, exception, vec![])
    }

    fn with_responses(
        targets: Value,
        fail_attach: bool,
        exception: Option<Value>,
        overrides: Vec<(&'static str, usize, Value)>,
    ) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let worker = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            let stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e) if e.kind() == ErrorKind::WouldBlock && Instant::now() < deadline => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(e) => panic!("fixture accept: {e}"),
                }
            };
            // Windows inherits the listener's nonblocking mode on accept.
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut socket = tungstenite::accept(stream).unwrap();
            let mut seen = Vec::new();
            let mut attached = String::new();
            loop {
                let raw = match socket.read() {
                    Ok(Message::Text(text)) => text,
                    Ok(Message::Close(_)) => break,
                    Ok(_) => continue,
                    Err(
                        tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed,
                    )
                    | Err(tungstenite::Error::Protocol(
                        ProtocolError::ResetWithoutClosingHandshake,
                    )) => break,
                    Err(tungstenite::Error::Io(e))
                        if matches!(
                            e.kind(),
                            ErrorKind::ConnectionReset | ErrorKind::UnexpectedEof
                        ) =>
                    {
                        break;
                    }
                    Err(e) => panic!("fixture read: {e}"),
                };
                let request: Value = serde_json::from_str(&raw).unwrap();
                seen.push(request.clone());
                let method = request["method"].as_str().unwrap();
                let result = match method {
                    "Target.getTargets" => json!({"targetInfos":targets}),
                    "Target.createTarget" => {
                        assert_eq!(request["params"], json!({"url":"about:blank"}));
                        json!({"targetId":"created"})
                    }
                    "Target.attachToTarget" => {
                        assert_eq!(request["params"]["flatten"], true);
                        if fail_attach {
                            socket
                                .send(Message::Text(
                                    json!({"id":request["id"],"error":{
                                        "code":-32602,"message":"No target with given id found"
                                    }})
                                    .to_string()
                                    .into(),
                                ))
                                .unwrap();
                            continue;
                        }
                        attached = request["params"]["targetId"].as_str().unwrap().to_owned();
                        json!({"sessionId":format!("attached-{attached}")})
                    }
                    _ => {
                        assert!(!attached.is_empty());
                        assert_eq!(request["sessionId"], format!("attached-{attached}"));
                        let occurrence = seen.iter().filter(|r| r["method"] == method).count();
                        if let Some((_, _, response)) = overrides
                            .iter()
                            .find(|(name, count, _)| *name == method && *count == occurrence)
                        {
                            let mut response = response.clone();
                            response["id"] = request["id"].clone();
                            socket
                                .send(Message::Text(response.to_string().into()))
                                .unwrap();
                            continue;
                        }
                        match method {
                            "Page.enable"
                            | "Runtime.enable"
                            | "Runtime.releaseObject"
                            | "Input.dispatchMouseEvent" => json!({}),
                            "Runtime.callFunctionOn" => {
                                json!({"result":{"value":{"x":100,"y":80}}})
                            }
                            "Page.navigate" => json!({"frameId":"frame"}),
                            "Page.getFrameTree" => json!({"selectedTarget":attached}),
                            "Page.getLayoutMetrics" => {
                                json!({"cssContentSize":{"width":10,"height":10}})
                            }
                            "Page.captureScreenshot" | "Page.printToPDF" => {
                                json!({"data":"YXJ0aWZhY3Q="})
                            }
                            "Runtime.evaluate" => {
                                if let Some(details) = &exception {
                                    socket
                                        .send(Message::Text(
                                            json!({
                                                "id":request["id"], "result":{
                                                    "result":{"type":"object","subtype":"error"},
                                                    "exceptionDetails":details
                                                }
                                            })
                                            .to_string()
                                            .into(),
                                        ))
                                        .unwrap();
                                    continue;
                                }
                                let expression = request["params"]["expression"].as_str().unwrap();
                                let value = match expression {
                                    "document.readyState" => json!("complete"),
                                    "location.href" => {
                                        json!(format!("https://example.test/{attached}"))
                                    }
                                    "document.title" => json!(attached),
                                    value if value.starts_with("(()=>({url:") => {
                                        json!({"url":format!("https://example.test/{attached}")})
                                    }
                                    value if value.contains("const visible=") => {
                                        json!([{"text":"Saved"}])
                                    }
                                    _ => json!(true),
                                };
                                if request["params"]["returnByValue"] == false {
                                    json!({"result":{"type":"object","subtype":"node","objectId":"element"}})
                                } else {
                                    json!({"result":{"value":value}})
                                }
                            }
                            _ => panic!("unexpected method: {method}"),
                        }
                    }
                };
                socket
                    .send(Message::Text(
                        json!({"id":request["id"],"result":result})
                            .to_string()
                            .into(),
                    ))
                    .unwrap();
            }
            seen
        });
        Self { url, worker }
    }

    fn finish(self) -> Vec<Value> {
        self.worker.join().unwrap()
    }
}

fn api(websocket: &str) -> MockServer {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST)
            .path("/instance/session")
            .json_body_partial(r#"{"session_id":"browser"}"#);
        then.status(200)
            .json_body(json!({"session_id":"browser","status":"active","ws":websocket}));
    });
    server
}

fn two_pages() -> Value {
    json!([
        {"targetId":"home","type":"page"},
        {"targetId":"wanted","type":"page"},
        {"targetId":"worker","type":"service_worker"}
    ])
}

fn click_fixture(
    overrides: Vec<(&'static str, usize, Value)>,
) -> (std::process::Output, Vec<Value>) {
    let directory = tempfile::tempdir().unwrap();
    let peer = Peer::with_responses(two_pages(), false, None, overrides);
    let server = api(&peer.url);
    let output = support::cli(
        &server.base_url(),
        directory.path(),
        &[
            "action",
            "click",
            "--session-id",
            "browser",
            "--target-id",
            "wanted",
            "--selector",
            r#"[data-name='a"b']"#,
        ],
    );
    (output, peer.finish())
}

fn click_events(seen: &[Value]) -> Vec<Value> {
    seen.iter()
        .filter(|r| r["method"] == "Input.dispatchMouseEvent")
        .map(|r| r["params"].clone())
        .collect()
}

#[test]
fn click_uses_native_input_and_rechecks_the_same_dom_object_after_hover() {
    let (output, seen) = click_fixture(vec![]);
    assert_eq!(support::data(output), true);
    assert_eq!(
        seen.iter()
            .skip(4)
            .map(|r| r["method"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "Runtime.evaluate",
            "Runtime.callFunctionOn",
            "Input.dispatchMouseEvent",
            "Runtime.callFunctionOn",
            "Input.dispatchMouseEvent",
            "Input.dispatchMouseEvent",
            "Runtime.releaseObject"
        ]
    );
    let lookup = &seen[4]["params"];
    assert_eq!(lookup["returnByValue"], false);
    assert!(
        lookup["expression"]
            .as_str()
            .unwrap()
            .contains(&serde_json::to_string(r#"[data-name='a"b']"#).unwrap())
    );
    for request in &seen {
        assert!(request["params"].get("userGesture").is_none());
        for field in ["expression", "functionDeclaration"] {
            assert!(
                !request["params"][field]
                    .as_str()
                    .unwrap_or("")
                    .contains(".click(")
            );
        }
    }
    assert_eq!(seen[5]["params"]["objectId"], "element");
    assert_eq!(seen[5]["params"]["arguments"], json!([{"value":null}]));
    assert_eq!(seen[7]["params"]["objectId"], "element");
    assert_eq!(
        seen[7]["params"]["arguments"],
        json!([{"value":{"x":100.0,"y":80.0}}])
    );
    let events = click_events(&seen);
    for (event, (kind, button, buttons, count)) in events.iter().zip([
        ("mouseMoved", "none", 0, 0),
        ("mousePressed", "left", 1, 1),
        ("mouseReleased", "left", 0, 1),
    ]) {
        assert_eq!(
            *event,
            json!({"type":kind,"x":100.0,"y":80.0,"button":button,"buttons":buttons,
            "clickCount":count,"pointerType":"mouse","modifiers":0})
        );
    }
    assert_eq!(
        seen.last().unwrap()["params"],
        json!({"objectId":"element"})
    );
}

#[test]
fn click_probe_errors_release_the_object_without_pressing() {
    for (occurrence, response, expected, expected_moves) in [
        (
            1,
            json!({"result":{"result":{"value":{"x":-1,"y":80}}}}),
            "invalid coordinates",
            0,
        ),
        (
            1,
            json!({"result":{"result":{"value":{"x":100}}}}),
            "invalid coordinates",
            0,
        ),
        (
            1,
            json!({"result":{"result":{"value":{"x":"NaN","y":80}}}}),
            "invalid coordinates",
            0,
        ),
        (
            1,
            json!({"result":{"exceptionDetails":{"exception":{"description":"Error: covered\n    at private-stack"}}}}),
            "Error: covered",
            0,
        ),
        (
            2,
            json!({"result":{"exceptionDetails":{"exception":{"description":"Error: detached"}}}}),
            "Error: detached",
            1,
        ),
        (
            2,
            json!({"error":{"code":-32000,"message":"context destroyed"}}),
            "context destroyed",
            1,
        ),
        (
            2,
            json!({"result":{"result":{"value":{"x":200,"y":80}}}}),
            "changed after hover",
            1,
        ),
    ] {
        let (output, seen) = click_fixture(vec![("Runtime.callFunctionOn", occurrence, response)]);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["error"], "cdp_error");
        assert!(
            error["message"].as_str().unwrap().contains(expected),
            "{error}"
        );
        assert!(!error["message"].as_str().unwrap().contains("private-stack"));
        let events = click_events(&seen);
        assert_eq!(events.len(), expected_moves);
        assert!(events.iter().all(|e| e["type"] == "mouseMoved"));
        assert_eq!(seen.last().unwrap()["method"], "Runtime.releaseObject");
    }
}

#[test]
fn invalid_click_object_is_rejected_before_input() {
    for remote in [json!({"type":"undefined"}), json!({"objectId":""})] {
        let (output, seen) = click_fixture(vec![(
            "Runtime.evaluate",
            1,
            json!({"result":{"result":remote}}),
        )]);
        assert_eq!(output.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&output.stderr).contains("missing objectId"));
        assert!(click_events(&seen).is_empty());
        assert_eq!(seen.last().unwrap()["method"], "Runtime.evaluate");
    }
}

#[test]
fn click_input_errors_are_propagated_without_repeating_press() {
    for occurrence in 1..=3 {
        let (output, seen) = click_fixture(vec![
            (
                "Input.dispatchMouseEvent",
                occurrence,
                json!({"error":{"code":-32000,"message":"input failure"}}),
            ),
            (
                "Runtime.releaseObject",
                1,
                json!({"error":{"code":-32000,"message":"cleanup failure"}}),
            ),
        ]);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["message"], "CDP command failed: input failure");
        let events = click_events(&seen);
        assert_eq!(events.len(), if occurrence == 1 { 1 } else { 3 });
        if occurrence > 1 {
            assert_eq!(events[1]["type"], "mousePressed");
            assert_eq!(events[2]["type"], "mouseReleased");
        }
        assert_eq!(seen.last().unwrap()["method"], "Runtime.releaseObject");
    }
}

#[test]
fn click_keeps_the_first_input_error_when_release_also_fails() {
    let (output, seen) = click_fixture(vec![
        (
            "Input.dispatchMouseEvent",
            2,
            json!({"error":{"code":-32000,"message":"press failure"}}),
        ),
        (
            "Input.dispatchMouseEvent",
            3,
            json!({"error":{"code":-32000,"message":"release failure"}}),
        ),
    ]);
    assert_eq!(output.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["message"], "CDP command failed: press failure");
    assert_eq!(click_events(&seen).len(), 3);
}

#[test]
fn click_cleanup_failure_does_not_mask_successful_input() {
    let (output, seen) = click_fixture(vec![(
        "Runtime.releaseObject",
        1,
        json!({"error":{"code":-32000,"message":"context already destroyed"}}),
    )]);
    assert_eq!(support::data(output), true);
    assert_eq!(click_events(&seen).len(), 3);
}

#[test]
fn evaluation_exception_details_reach_cli_without_changing_failure_contract() {
    let directory = tempfile::tempdir().unwrap();
    for args in [
        vec![
            "eval",
            "--expression",
            "document.querySelector('#missing').click()",
        ],
        vec!["click", "--selector", "#missing"],
        vec!["fill", "--selector", "#missing", "--value", "hello"],
    ] {
        let peer = Peer::with_exception(
            two_pages(),
            false,
            Some(json!({
                "text":"Uncaught", "lineNumber":2, "columnNumber":4,
                "url":"https://example.test/?token=private-source-url",
                "exception":{
                    "className":"TypeError", "objectId":"private-object-id",
                    "description":"TypeError: Cannot read properties of null (reading 'click')\n    at https://example.test/?token=private-stack-url:3:5"
                },
                "stackTrace":{"callFrames":[{"url":"private-stack-frame"}]}
            })),
        );
        let server = api(&peer.url);
        let mut arguments = vec!["action"];
        arguments.extend(args);
        arguments.extend(["--session-id", "browser", "--target-id", "wanted"]);
        let output = support::cli(&server.base_url(), directory.path(), &arguments);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(
            error,
            json!({"ok":false,"error":"cdp_error",
            "message":"CDP command failed: TypeError: Cannot read properties of null (reading 'click') (line 3, column 5)"})
        );
        let seen = peer.finish();
        assert_eq!(
            seen.iter()
                .filter(|r| r["method"] == "Runtime.evaluate")
                .count(),
            1
        );
    }
}

#[test]
fn sdk_evaluation_preserves_syntax_errors() {
    if support::isolated_test(
        "sdk_evaluation_preserves_syntax_errors",
        Duration::from_secs(20),
    ) {
        return;
    }
    let peer = Peer::with_exception(
        two_pages(),
        false,
        Some(json!({
            "text":"Uncaught", "lineNumber":0, "columnNumber":20,
            "exception":{"className":"SyntaxError", "description":"SyntaxError: Invalid regular expression: missing /"}
        })),
    );
    let error = {
        let mut cdp = Cdp::connect_to_target(&peer.url, "wanted").unwrap();
        cdp.evaluate("invalid syntax").unwrap_err()
    };
    assert_eq!(
        error.to_string(),
        "CDP command failed: SyntaxError: Invalid regular expression: missing / (line 1, column 21)"
    );
    peer.finish();
}

#[test]
fn sdk_selects_the_requested_target_regardless_of_enumeration_order() {
    if support::isolated_test(
        "sdk_selects_the_requested_target_regardless_of_enumeration_order",
        Duration::from_secs(20),
    ) {
        return;
    }
    for reversed in [false, true] {
        let mut targets = two_pages();
        if reversed {
            targets.as_array_mut().unwrap().reverse();
        }
        let peer = Peer::start(targets, false);
        {
            let mut cdp = Cdp::connect_to_target(&peer.url, "wanted").unwrap();
            assert_eq!(
                cdp.evaluate("location.href").unwrap(),
                "https://example.test/wanted"
            );
        }
        let seen = peer.finish();
        assert_eq!(seen[1]["params"]["targetId"], "wanted");
        assert!(seen.iter().all(|v| v["method"] != "Target.createTarget"));
    }
}

#[test]
fn every_cli_action_routes_to_the_explicit_target_and_preserves_json_output() {
    let directory = tempfile::tempdir().unwrap();
    for arguments in [
        vec!["open-url", "--url", "https://example.test/wanted"],
        vec!["eval", "--expression", "location.href"],
        vec!["wait-selector", "--selector", "#ready"],
        vec!["wait-text", "--text", "Saved"],
        vec!["click", "--selector", "#button"],
        vec!["fill", "--selector", "#input", "--value", "hello"],
        vec!["screenshot", "--path", "image.png", "--full-page"],
        vec!["pdf", "--path", "page.pdf", "--print-background"],
        vec!["snapshot"],
        vec!["raw", "--method", "Page.getFrameTree"],
    ] {
        let peer = Peer::start(two_pages(), false);
        let server = api(&peer.url);
        let mut args = vec!["action"];
        args.extend(&arguments);
        args.extend(["--session-id", "browser", "--target-id", "wanted"]);
        let result = support::data(support::cli(&server.base_url(), directory.path(), &args));
        match arguments[0] {
            "open-url" | "snapshot" => assert_eq!(result["url"], "https://example.test/wanted"),
            "eval" => assert_eq!(result, "https://example.test/wanted"),
            "wait-selector" | "wait-text" => assert_eq!(result["found"], true),
            "click" | "fill" => assert_eq!(result, true),
            "screenshot" | "pdf" => {
                assert_eq!(result["bytes"], 8);
                assert_eq!(
                    fs::read(directory.path().join(arguments[2])).unwrap(),
                    b"artifact"
                );
            }
            "raw" => assert_eq!(result["selectedTarget"], "wanted"),
            _ => unreachable!(),
        }
        let seen = peer.finish();
        assert_eq!(seen[1]["params"]["targetId"], "wanted");
        assert!(seen.iter().all(|v| v["method"] != "Target.createTarget"));
        assert!(
            seen.iter()
                .skip(2)
                .all(|v| v["sessionId"] == "attached-wanted")
        );
    }
}

#[test]
fn invalid_cli_targets_fail_without_attaching_creating_or_overwriting_files() {
    for (targets, id, error) in [
        (two_pages(), "closed-or-other-session", "not_found"),
        (json!([]), "missing", "not_found"),
        (two_pages(), "worker", "configuration_error"),
    ] {
        let peer = Peer::start(targets, false);
        let server = api(&peer.url);
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("existing.png"), b"do not replace").unwrap();
        let output = support::cli(
            &server.base_url(),
            directory.path(),
            &[
                "action",
                "screenshot",
                "--session-id",
                "browser",
                "--target-id",
                id,
                "--path",
                "existing.png",
            ],
        );
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let result: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(result["ok"], false);
        assert_eq!(result["error"], error);
        assert!(
            result["message"]
                .as_str()
                .unwrap()
                .contains("session targets")
        );
        assert_eq!(
            fs::read(directory.path().join("existing.png")).unwrap(),
            b"do not replace"
        );
        let seen = peer.finish();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0]["method"], "Target.getTargets");
    }
}

#[test]
fn target_closed_between_listing_and_attach_fails_without_fallback() {
    let peer = Peer::start(two_pages(), true);
    let server = api(&peer.url);
    let directory = tempfile::tempdir().unwrap();
    let output = support::cli(
        &server.base_url(),
        directory.path(),
        &[
            "action",
            "click",
            "--session-id",
            "browser",
            "--target-id",
            "wanted",
            "--selector",
            "#button",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let result: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(result["error"], "cdp_error");
    let seen = peer.finish();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[1]["method"], "Target.attachToTarget");
    assert_eq!(seen[1]["params"]["targetId"], "wanted");
}

#[test]
fn default_cli_still_uses_first_page_or_creates_blank_when_no_pages_exist() {
    for (targets, selected, creates) in [(two_pages(), "home", false), (json!([]), "created", true)]
    {
        let peer = Peer::start(targets, false);
        let server = api(&peer.url);
        let directory = tempfile::tempdir().unwrap();
        let result = support::data(support::cli(
            &server.base_url(),
            directory.path(),
            &[
                "action",
                "eval",
                "--session-id",
                "browser",
                "--expression",
                "location.href",
            ],
        ));
        assert_eq!(result, format!("https://example.test/{selected}"));
        let seen = peer.finish();
        assert_eq!(
            seen.iter().any(|v| v["method"] == "Target.createTarget"),
            creates
        );
        let attached = seen
            .iter()
            .find(|v| v["method"] == "Target.attachToTarget")
            .unwrap();
        assert_eq!(attached["params"]["targetId"], selected);
    }
}

#[test]
fn connection_timeout_reports_json_failure_without_sending_an_action() {
    use std::io::Read;
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let url = format!(
        "ws://{}/?token=private-secret",
        listener.local_addr().unwrap()
    );
    let server = api(&url);
    let worker = thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(e) if e.kind() == ErrorKind::WouldBlock && Instant::now() < until => {
                    thread::sleep(Duration::from_millis(10))
                }
                Err(e) => panic!("fixture accept: {e}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).unwrap();
        // Only a WebSocket HTTP upgrade was sent; no CDP action frame followed.
        assert!(bytes.starts_with(b"GET /"));
        assert!(bytes.ends_with(b"\r\n\r\n"));
        assert!(!String::from_utf8_lossy(&bytes).contains("Page.navigate"));
    });
    let directory = tempfile::tempdir().unwrap();
    let start = Instant::now();
    let output = support::cli(
        &server.base_url(),
        directory.path(),
        &[
            "action",
            "open-url",
            "--session-id",
            "browser",
            "--url",
            "https://example.test/",
        ],
    );
    assert!(start.elapsed() < Duration::from_secs(19));
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let envelope: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(
        envelope,
        json!({"ok":false,"error":"timeout",
        "message":"request timed out: CDP connection (stage: websocket_handshake, budget: 15s)"})
    );
    worker.join().unwrap();
}
