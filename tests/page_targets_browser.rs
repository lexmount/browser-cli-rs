//! Opt-in end-to-end regression with a separate headless Chrome profile.
//! No GUI, cloud session, real site, or user credentials are used.
mod support;

use httpmock::{Method::GET, Method::POST, MockServer};
use lexmount_browser::cdp::Cdp;
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

const HOME: &str = r#"<!doctype html><title>Search fixture</title>
<input id="query"><button id="search">Search</button>
<script>document.querySelector('#search').onclick = event => {
  window.clickEvidence = {trusted: event.isTrusted, active: navigator.userActivation.isActive};
  const popup = window.open('/result?q=' + encodeURIComponent(document.querySelector('#query').value), '_blank');
  window.clickEvidence.opened = popup !== null;
};</script>"#;

const RESULT: &str = r#"<!doctype html><title>Result fixture</title>
<p id="result">Search result</p><input id="query"><button id="save">Save</button>
<script>document.querySelector('#save').onclick = () => {
  document.querySelector('#result').textContent = document.querySelector('#query').value;
};</script>"#;

struct Browser {
    process: Child,
    _profile: tempfile::TempDir,
    websocket: String,
    http: String,
}

impl Browser {
    fn start(executable: &Path) -> Self {
        let profile = tempfile::Builder::new()
            .prefix("browser-cli-targets-")
            .tempdir()
            .unwrap();
        let mut command = Command::new(executable);
        command.args([
            "--headless",
            "--remote-debugging-port=0",
            "--remote-debugging-address=127.0.0.1",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-background-networking",
            "--disable-component-update",
            "--disable-sync",
            "--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE localhost, EXCLUDE 127.0.0.1",
            "--no-proxy-server",
        ]);
        command.arg(format!("--user-data-dir={}", profile.path().display()));
        command.arg("about:blank");
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let process = command.spawn().unwrap();
        let mut browser = Self {
            process,
            _profile: profile,
            websocket: String::new(),
            http: String::new(),
        };
        let deadline = Instant::now() + Duration::from_secs(20);
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        loop {
            if let Ok(port_file) =
                fs::read_to_string(browser._profile.path().join("DevToolsActivePort"))
            {
                let mut lines = port_file.lines();
                // The file can briefly be empty/partial while Chrome starts.
                if let (Some(port), Some(path)) = (lines.next(), lines.next())
                    && let Ok(port) = port.parse::<u16>()
                    && path.starts_with("/devtools/browser/")
                {
                    browser.websocket = format!("ws://127.0.0.1:{port}{path}");
                    browser.http = format!("http://127.0.0.1:{port}");
                    // Wait for the initial page too, avoiding a racing blank
                    // target creation by Cdp::connect during browser startup.
                    if let Ok(response) = client.get(format!("{}/json", browser.http)).send()
                        && let Ok(targets) = response.json::<Vec<Value>>()
                        && targets.iter().any(|t| t["type"] == "page")
                    {
                        return browser;
                    }
                }
            }
            assert!(
                browser.process.try_wait().unwrap().is_none(),
                "headless browser exited"
            );
            assert!(Instant::now() < deadline, "headless startup timed out");
            thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        // Close only this test's isolated browser, including its renderer children.
        if !self.websocket.is_empty()
            && let Ok((mut socket, _)) = tungstenite::connect(self.websocket.as_str())
        {
            let _ = socket.send(tungstenite::Message::Text(
                json!({"id":1,"method":"Browser.close"}).to_string().into(),
            ));
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.process.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

fn pages(cdp: &mut Cdp) -> Vec<Value> {
    cdp.command_root("Target.getTargets", json!({})).unwrap()["targetInfos"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["type"] == "page")
        .cloned()
        .collect()
}

#[test]
#[ignore = "requires BROWSER_CLI_TEST_CHROME pointing to a Chrome/Chromium executable"]
fn optional_pointer_metadata_getters_cannot_block_native_click() {
    if support::isolated_test(
        "optional_pointer_metadata_getters_cannot_block_native_click",
        Duration::from_secs(120),
    ) {
        return;
    }
    let chromium = std::env::var_os("BROWSER_CLI_TEST_CHROME").unwrap();
    let browser = Browser::start(Path::new(&chromium));
    let directory = tempfile::tempdir().unwrap();
    let api = MockServer::start();
    api.mock(|when, then| {
        when.method(POST).path("/instance/session");
        then.status(200)
            .json_body(json!({"session_id":"browser","status":"active","ws":browser.websocket}));
    });
    let mut cdp = Cdp::connect(&browser.websocket).unwrap();
    let target_id = cdp.target_id().to_owned();
    let all_metadata = [
        "viewport_width",
        "viewport_height",
        "device_scale",
        "page_scale",
        "page_generation",
    ];
    let cases: Vec<(&str, &str, Vec<&str>)> = vec![
        (
            "visualViewport getter",
            "Object.defineProperty(window,'visualViewport',{configurable:true,get(){throw new Error('optional viewport unavailable')}})",
            vec!["page_scale"],
        ),
        (
            "visualViewport.scale getter",
            "Object.defineProperty(window.visualViewport,'scale',{configurable:true,get(){throw new Error('optional scale unavailable')}})",
            vec!["page_scale"],
        ),
        (
            "devicePixelRatio getter",
            "Object.defineProperty(window,'devicePixelRatio',{configurable:true,get(){throw new Error('optional device scale unavailable')}})",
            vec!["device_scale"],
        ),
        (
            "timeOrigin getter",
            "Object.defineProperty(performance,'timeOrigin',{configurable:true,get(){throw new Error('optional generation unavailable')}})",
            vec!["page_generation"],
        ),
        (
            "all optional getters",
            "for(const key of ['innerWidth','innerHeight','devicePixelRatio','visualViewport'])Object.defineProperty(window,key,{configurable:true,get(){throw new Error('optional metadata unavailable')}});Object.defineProperty(performance,'timeOrigin',{configurable:true,get(){throw new Error('optional generation unavailable')}})",
            all_metadata.to_vec(),
        ),
        (
            "invalid optional values",
            "Object.defineProperty(window,'innerWidth',{configurable:true,get(){return 'not-a-number'}});Object.defineProperty(window,'innerHeight',{configurable:true,get(){return Infinity}});Object.defineProperty(window,'devicePixelRatio',{configurable:true,get(){return 0}});Object.defineProperty(window,'visualViewport',{configurable:true,get(){return {scale:-1}}});Object.defineProperty(performance,'timeOrigin',{configurable:true,get(){return 'private-not-a-generation'}})",
            all_metadata.to_vec(),
        ),
    ];
    for (name, install_getters, missing_fields) in cases {
        cdp.navigate("about:blank", Duration::from_secs(5)).unwrap();
        let install_getters = install_getters
            .replace("throw new Error", "window.optionalReads++;throw new Error")
            .replace("return ", "window.optionalReads++;return ");
        assert_eq!(cdp.evaluate(&format!(
            "document.body.innerHTML='<button id=target>Click</button>';window.clicks=0;window.optionalReads=0;document.querySelector('#target').onclick=()=>window.clicks++;{install_getters};true"
        )).unwrap(), true, "{name}");
        let events = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&events);
        cdp.set_pointer_observer(move |pointer| observed.lock().unwrap().push(pointer.clone()));
        assert_eq!(cdp.click("#target").unwrap(), true, "{name}");
        assert_eq!(cdp.evaluate("window.clicks").unwrap(), 1, "{name}");
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 3, "{name}");
        for event in events.iter() {
            assert!(event.x.is_finite() && event.x >= 0.0, "{name}");
            assert!(event.y.is_finite() && event.y >= 0.0, "{name}");
            let value = serde_json::to_value(event).unwrap();
            for field in &missing_fields {
                assert!(
                    value[*field].is_null(),
                    "{name}: {field} must remain unknown"
                );
            }
        }
        // The ordinary CLI path explicitly disables desktop observations.
        // Optional getters must not affect its original click/JSON behavior.
        let optional_reads = cdp.evaluate("window.optionalReads").unwrap();
        assert!(
            optional_reads.as_u64().unwrap() > 0,
            "{name}: SDK observer must exercise getters"
        );
        let output = support::command(
            &api.base_url(),
            directory.path(),
            &[
                "action",
                "click",
                "--session-id",
                "browser",
                "--target-id",
                &target_id,
                "--selector",
                "#target",
            ],
        )
        .env("LEXMOUNT_APP_EVENTS", "off")
        .output()
        .unwrap();
        assert_eq!(support::data(output), true, "{name}");
        assert_eq!(cdp.evaluate("window.clicks").unwrap(), 2, "{name}");
        assert_eq!(
            cdp.evaluate("window.optionalReads").unwrap(),
            optional_reads,
            "{name}: events-off CLI must not read optional metadata getters"
        );
    }
}

#[test]
#[ignore = "requires BROWSER_CLI_TEST_CHROME pointing to a Chrome/Chromium executable"]
fn native_click_checks_actionability_and_retains_the_original_element() {
    if support::isolated_test(
        "native_click_checks_actionability_and_retains_the_original_element",
        Duration::from_secs(120),
    ) {
        return;
    }
    let chromium = std::env::var_os("BROWSER_CLI_TEST_CHROME").unwrap();
    let browser = Browser::start(Path::new(&chromium));
    let directory = tempfile::tempdir().unwrap();
    let api = MockServer::start();
    api.mock(|when, then| {
        when.method(POST).path("/instance/session");
        then.status(200)
            .json_body(json!({"session_id":"browser","status":"active","ws":browser.websocket}));
    });
    let mut observer = Cdp::connect(&browser.websocket).unwrap();
    let targets = pages(&mut observer);
    let target = targets[0]["targetId"].as_str().unwrap();
    let cases = [
        ("<button id=target>Click</button>", "#target", None),
        (
            "<button id=target><span>Child</span></button>",
            "#target",
            None,
        ),
        (
            "<div style='height:1600px'></div><button id=target>Below fold</button>",
            "#target",
            None,
        ),
        (
            "<div style='height:80px;overflow:auto'><div style='height:800px'></div><button id=target>Nested scroll</button></div>",
            "#target",
            None,
        ),
        (
            "<button id=target style='transform:scale(1.3)'>Transformed</button>",
            "#target",
            None,
        ),
        (
            "<button id=target data-label='a&quot;b'>Quoted selector</button>",
            r#"[data-label='a"b']"#,
            None,
        ),
        (
            "<fieldset disabled><legend><button id=target>Enabled first legend</button></legend></fieldset>",
            "#target",
            None,
        ),
        (
            "<button id=target disabled>Disabled</button>",
            "#target",
            Some("disabled"),
        ),
        (
            "<fieldset disabled><button id=target>Disabled by fieldset</button></fieldset>",
            "#target",
            Some("disabled"),
        ),
        (
            "<button disabled><span id=target>Disabled parent button</span></button>",
            "#target",
            Some("disabled"),
        ),
        (
            "<div inert><button id=target>Inert</button></div>",
            "#target",
            Some("inert"),
        ),
        (
            "<div aria-disabled=true><button id=target>ARIA disabled</button></div>",
            "#target",
            Some("disabled"),
        ),
        (
            "<button id=target hidden>Hidden</button>",
            "#target",
            Some("not visible"),
        ),
        (
            "<button id=target style='visibility:hidden'>Invisible</button>",
            "#target",
            Some("not visible"),
        ),
        (
            "<div hidden><button id=target>Hidden parent</button></div>",
            "#target",
            Some("no visible area"),
        ),
        (
            "<button id=target style='pointer-events:none'>No pointer</button>",
            "#target",
            Some("cannot receive pointer"),
        ),
        (
            "<button id=target>Covered</button><div id=overlay style='position:fixed;inset:0;z-index:99'>Overlay</div>",
            "#target",
            Some("covered"),
        ),
        (
            "<button id=target onmouseover='this.disabled=true'>Disabled after hover</button>",
            "#target",
            Some("disabled"),
        ),
        (
            "<button id=target onmouseover='this.replaceWith(this.cloneNode(true))'>Replaced after hover</button>",
            "#target",
            Some("detached"),
        ),
        (
            "<button id=target onmouseover=\"this.style.transform='translateX(300px)'\">Moved after hover</button>",
            "#target",
            Some("moved after hover"),
        ),
        (
            "<button id=target onmouseover=\"document.querySelector('#overlay').hidden=false\">Hover overlay</button><div id=overlay hidden style='position:fixed;inset:0;z-index:99'>Overlay</div>",
            "#target",
            Some("covered"),
        ),
        ("<p>No target</p>", "#target", Some("selector not found")),
        (
            "<button id=target>Invalid selector</button>",
            "[",
            Some("SyntaxError"),
        ),
    ];
    for (html, selector, expected_error) in cases {
        // Reset both document and pointer so mouseover is deterministic per case.
        observer
            .navigate("about:blank", Duration::from_secs(5))
            .unwrap();
        observer
            .command(
                "Input.dispatchMouseEvent",
                json!({"type":"mouseMoved","x":0,"y":0}),
            )
            .unwrap();
        observer.evaluate(&format!(
            "document.body.innerHTML={}; window.clicks=[]; window.presses=0; document.addEventListener('pointerdown',()=>window.presses++,true); document.addEventListener('click',e=>window.clicks.push({{id:e.target.id,trusted:e.isTrusted,active:navigator.userActivation.isActive}}),true); true",
            serde_json::to_string(html).unwrap()
        )).unwrap();
        let output = support::cli(
            &api.base_url(),
            directory.path(),
            &[
                "action",
                "click",
                "--session-id",
                "browser",
                "--target-id",
                target,
                "--selector",
                selector,
            ],
        );
        let events = observer
            .evaluate("({clicks:window.clicks,presses:window.presses})")
            .unwrap();
        if let Some(expected) = expected_error {
            assert_eq!(output.status.code(), Some(1), "{html}");
            assert!(output.stdout.is_empty());
            let error: Value = serde_json::from_slice(&output.stderr).unwrap();
            assert_eq!(error["error"], "cdp_error");
            assert!(
                error["message"].as_str().unwrap().contains(expected),
                "{html}: {error}"
            );
            assert_eq!(
                events,
                json!({"clicks":[],"presses":0}),
                "must not click target/overlay/replacement: {html}"
            );
        } else {
            assert_eq!(support::data(output), true, "{html}");
            assert_eq!(events["presses"], 1, "{html}");
            let clicks = events["clicks"].as_array().unwrap();
            assert_eq!(clicks.len(), 1, "{html}");
            assert_eq!(clicks[0]["trusted"], true, "{html}");
            assert_eq!(clicks[0]["active"], true, "{html}");
        }
        assert_eq!(observer.evaluate("location.href").unwrap(), "about:blank");
        assert_eq!(pages(&mut observer).len(), 1);
    }

    // Arbitrary evaluation stays untrusted and gains no user activation.
    observer
        .navigate("about:blank", Duration::from_secs(5))
        .unwrap();
    assert_eq!(observer.evaluate(
        "document.body.innerHTML='<button>Test</button>'; let evidence; const b=document.querySelector('button'); b.onclick=e=>evidence={trusted:e.isTrusted,active:navigator.userActivation.isActive}; b.click(); evidence"
    ).unwrap(), json!({"trusted":false,"active":false}));

    // Same-tab navigation can destroy the remote DOM object before cleanup.
    // It must still produce the unchanged successful click result.
    let fixture = MockServer::start();
    fixture.mock(|when, then| {
        when.method(GET).path("/next");
        then.status(200)
            .body("<!doctype html><title>Next page</title>");
    });
    let next_url = fixture.url("/next");
    observer.evaluate(&format!(
        "document.body.innerHTML='<a id=target>Next page</a>'; document.querySelector('#target').href={}; true",
        serde_json::to_string(&next_url).unwrap()
    )).unwrap();
    assert_eq!(
        support::data(support::cli(
            &api.base_url(),
            directory.path(),
            &[
                "action",
                "click",
                "--session-id",
                "browser",
                "--target-id",
                target,
                "--selector",
                "#target"
            ]
        )),
        true
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !pages(&mut observer)
        .iter()
        .any(|page| page["targetId"] == target && page["url"] == next_url)
    {
        assert!(
            Instant::now() < deadline,
            "same-page click did not navigate"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
#[ignore = "requires BROWSER_CLI_TEST_CHROME pointing to a Chrome/Chromium executable"]
fn javascript_errors_are_actionable_with_a_real_browser() {
    if support::isolated_test(
        "javascript_errors_are_actionable_with_a_real_browser",
        Duration::from_secs(120),
    ) {
        return;
    }
    let chromium = std::env::var_os("BROWSER_CLI_TEST_CHROME")
        .expect("set BROWSER_CLI_TEST_CHROME to a local Chrome/Chromium executable");
    let browser = Browser::start(Path::new(&chromium));
    let directory = tempfile::tempdir().unwrap();
    let api = MockServer::start();
    api.mock(|when, then| {
        when.method(POST).path("/instance/session");
        then.status(200)
            .json_body(json!({"session_id":"browser","status":"active","ws":browser.websocket}));
    });
    for (args, expected) in [
        (
            vec!["click", "--selector", "#missing"],
            "Error: selector not found",
        ),
        (
            vec!["fill", "--selector", "#missing", "--value", "test"],
            "Error: selector not found",
        ),
        (
            vec![
                "eval",
                "--expression",
                "document.querySelector('#missing').click()",
            ],
            "TypeError:",
        ),
        (vec!["eval", "--expression", "/[/"], "SyntaxError:"),
        (
            vec!["eval", "--expression", "throw 'not ready'"],
            "not ready",
        ),
        (
            vec![
                "eval",
                "--expression",
                "Promise.reject(new Error('not ready'))",
            ],
            "Error: not ready",
        ),
    ] {
        let mut arguments = vec!["action"];
        arguments.extend(args);
        arguments.extend(["--session-id", "browser"]);
        let output = support::cli(&api.base_url(), directory.path(), &arguments);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["ok"], false);
        assert_eq!(error["error"], "cdp_error");
        let message = error["message"].as_str().unwrap();
        assert!(message.contains(expected), "{message}");
        assert!(message.contains("(line "), "{message}");
        assert!(!message.contains('\n'), "stack should not be appended");
        assert_eq!(
            support::data(support::cli(
                &api.base_url(),
                directory.path(),
                &[
                    "action",
                    "eval",
                    "--session-id",
                    "browser",
                    "--expression",
                    "1+1"
                ]
            )),
            2
        );
    }
    let mut observer = Cdp::connect(&browser.websocket).unwrap();
    let version = observer
        .command_root("Browser.getVersion", json!({}))
        .unwrap();
    println!(
        "{}",
        json!({"browser":version["product"], "error_cases":6, "success_after_each_error":true})
    );
}

#[test]
#[ignore = "requires BROWSER_CLI_TEST_CHROME pointing to a Chrome/Chromium executable"]
fn search_popup_can_be_selected_across_cli_invocations_and_closed_safely() {
    if support::isolated_test(
        "search_popup_can_be_selected_across_cli_invocations_and_closed_safely",
        Duration::from_secs(120),
    ) {
        return;
    }
    let chromium = std::env::var_os("BROWSER_CLI_TEST_CHROME")
        .expect("set BROWSER_CLI_TEST_CHROME to a local Chrome/Chromium executable");
    let browser = Browser::start(Path::new(&chromium));
    let directory = tempfile::tempdir().unwrap();
    let fixture = MockServer::start();
    fixture.mock(|when, then| {
        when.method(GET).path("/home");
        then.status(200)
            .header("Content-Type", "text/html; charset=utf-8")
            .body(HOME);
    });
    fixture.mock(|when, then| {
        when.method(GET).path("/result");
        then.status(200)
            .header("Content-Type", "text/html; charset=utf-8")
            .body(RESULT);
    });
    let api = MockServer::start();
    api.mock(|when, then| {
        when.method(POST).path("/instance/session");
        then.status(200)
            .json_body(json!({"session_id":"browser","status":"active","ws":browser.websocket}));
    });
    let run = |args: &[&str]| support::data(support::cli(&api.base_url(), directory.path(), args));
    let mut observer = Cdp::connect(&browser.websocket).unwrap();
    let version = observer
        .command_root("Browser.getVersion", json!({}))
        .unwrap();
    let initial = pages(&mut observer);
    assert_eq!(initial.len(), 1);
    let home_id = initial[0]["targetId"].as_str().unwrap();
    let home_url = fixture.url("/home");
    // Legacy syntax still works for the initial single page.
    assert_eq!(
        run(&[
            "action",
            "open-url",
            "--session-id",
            "browser",
            "--url",
            &home_url
        ])["url"],
        home_url
    );
    run(&[
        "action",
        "wait-selector",
        "--session-id",
        "browser",
        "--target-id",
        home_id,
        "--selector",
        "#query",
    ]);
    run(&[
        "action",
        "fill",
        "--session-id",
        "browser",
        "--target-id",
        home_id,
        "--selector",
        "#query",
        "--value",
        "page target",
    ]);
    run(&[
        "action",
        "click",
        "--session-id",
        "browser",
        "--target-id",
        home_id,
        "--selector",
        "#search",
    ]);
    // Tab count alone is insufficient: headless-shell can allow untrusted popups.
    assert_eq!(
        observer.evaluate("window.clickEvidence").unwrap(),
        json!({"trusted":true,"active":true,"opened":true}),
        "click must deliver real browser input, not HTMLElement.click()"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let result = loop {
        let targets = pages(&mut observer);
        if let Some(result) = targets.iter().find(|v| {
            v["url"]
                .as_str()
                .is_some_and(|url| url.starts_with(&fixture.url("/result")))
        }) {
            break result.clone();
        }
        assert!(Instant::now() < deadline, "search popup did not open");
        thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(result["openerId"], home_id);
    let result_id = result["targetId"].as_str().unwrap();
    let result_url = result["url"].as_str().unwrap();

    // Reuse the real browser's /json listing as the local session-targets API.
    // This proves its page `id` is usable as --target-id, not a CDP sessionId.
    let listed: Value = reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
        .get(format!("{}/json", browser.http))
        .send()
        .unwrap()
        .json()
        .unwrap();
    api.mock(|when, then| {
        when.method(GET)
            .path("/json")
            .query_param("session_id", "browser");
        then.status(200).json_body(listed.clone());
    });
    let discovered = run(&["session", "targets", "--session-id", "browser"]);
    let selected = discovered
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["type"] == "page" && v["url"] == result_url)
        .unwrap();
    assert_eq!(selected["id"], result_id);
    let selected_id = selected["id"].as_str().unwrap();
    run(&[
        "action",
        "wait-selector",
        "--session-id",
        "browser",
        "--target-id",
        selected_id,
        "--selector",
        "#result",
    ]);
    for _ in 0..3 {
        assert_eq!(
            run(&[
                "action",
                "eval",
                "--session-id",
                "browser",
                "--target-id",
                selected_id,
                "--expression",
                "location.href"
            ]),
            result_url
        );
    }
    // An unrelated tab's lifecycle cannot change explicit action routing.
    let extra = observer
        .command_root("Target.createTarget", json!({"url":"about:blank"}))
        .unwrap();
    observer
        .command_root("Target.closeTarget", json!({"targetId":extra["targetId"]}))
        .unwrap();
    run(&[
        "action",
        "fill",
        "--session-id",
        "browser",
        "--target-id",
        selected_id,
        "--selector",
        "#query",
        "--value",
        "saved on result",
    ]);
    run(&[
        "action",
        "click",
        "--session-id",
        "browser",
        "--target-id",
        selected_id,
        "--selector",
        "#save",
    ]);
    let snapshot = run(&[
        "action",
        "snapshot",
        "--session-id",
        "browser",
        "--target-id",
        selected_id,
    ]);
    assert_eq!(snapshot["url"], result_url);
    assert!(
        snapshot["text"]
            .as_str()
            .unwrap()
            .contains("saved on result")
    );
    assert_eq!(
        observer
            .evaluate("document.querySelector('#query').value")
            .unwrap(),
        "page target"
    );
    assert_eq!(observer.evaluate("location.href").unwrap(), home_url);

    observer
        .command_root("Target.closeTarget", json!({"targetId":selected_id}))
        .unwrap();
    let output = support::cli(
        &api.base_url(),
        directory.path(),
        &[
            "action",
            "fill",
            "--session-id",
            "browser",
            "--target-id",
            selected_id,
            "--selector",
            "#query",
            "--value",
            "must not reach home",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"], "not_found");
    assert_eq!(
        observer
            .evaluate("document.querySelector('#query').value")
            .unwrap(),
        "page target"
    );
    let remaining = pages(&mut observer);
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0]["targetId"], home_id);
    println!(
        "{}",
        json!({"browser":version["product"],"protocol":version["protocolVersion"],
        "legacy_single_page":"passed","popup_discovery":"passed","explicit_reconnects":3,
        "unrelated_tab_lifecycle":"passed","result_page_mutation":"passed",
        "closed_target_no_fallback":"passed","source_page_unchanged":"passed"})
    );
}

#[test]
#[ignore = "requires BROWSER_CLI_TEST_CHROME pointing to a Chrome/Chromium executable"]
fn real_mouse_move_is_trusted_unpressed_and_aborts_small_hover_layout_changes() {
    if support::isolated_test(
        "real_mouse_move_is_trusted_unpressed_and_aborts_small_hover_layout_changes",
        Duration::from_secs(90),
    ) {
        return;
    }
    let executable = std::env::var_os("BROWSER_CLI_TEST_CHROME").unwrap();
    let browser = Browser::start(Path::new(&executable));
    let mut cdp = Cdp::connect(&browser.websocket).unwrap();
    // The startup page exists before macOS headless Chrome finishes sizing its
    // window. Pin this fixture's CSS viewport; production must still reject a
    // real resize between an observed origin and the next move.
    cdp.command(
        "Emulation.setDeviceMetricsOverride",
        json!({"width":900,"height":600,"deviceScaleFactor":1,"mobile":false}),
    )
    .unwrap();
    cdp.evaluate(r#"document.body.innerHTML='<button id=start style="position:fixed;left:40px;top:70px;width:100px;height:40px">Start</button><button id=finish style="position:fixed;left:350px;top:230px;width:140px;height:50px">Finish</button>';
      window.moves=[];window.presses=0;
      document.addEventListener('mousemove',e=>moves.push({x:e.clientX,y:e.clientY,trusted:e.isTrusted}));
      document.addEventListener('mousedown',()=>presses++);true"#).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut previous = Value::Null;
    let mut stable = 0;
    while stable < 3 {
        let current = cdp.evaluate("({ready:document.readyState,width:innerWidth,height:innerHeight,scale:visualViewport.scale,dpr:devicePixelRatio,generation:performance.timeOrigin})").unwrap();
        if current["ready"] == "complete" && current == previous {
            stable += 1;
        } else {
            stable = 0;
        }
        previous = current;
        assert!(Instant::now() < deadline, "fixture viewport did not settle");
        thread::sleep(Duration::from_millis(30));
    }
    let origin: lexmount_browser::cdp::PointerOrigin =
        serde_json::from_value(cdp.click_with_point("#start").unwrap()).unwrap();
    cdp.evaluate("window.moves=[];window.presses=0;true")
        .unwrap();
    let observed = Arc::new(Mutex::new(Vec::new()));
    let capture = Arc::clone(&observed);
    cdp.set_pointer_observer(move |event| capture.lock().unwrap().push(event.clone()));
    let result = cdp
        .move_pointer("#finish", &origin, Duration::from_millis(300))
        .unwrap();
    println!(
        "{}",
        json!({"isolated_move_samples":result["steps"],"elapsed_ms":result["elapsed_ms"],
               "requested_duration_ms":result["requested_duration_ms"]})
    );
    let evidence = cdp
        .evaluate(
            "({presses,moves:moves.length,trusted:moves.every(e=>e.trusted),last:moves.at(-1)})",
        )
        .unwrap();
    assert_eq!(evidence["presses"], 0);
    assert_eq!(evidence["trusted"], true);
    assert!(evidence["moves"].as_u64().unwrap() >= 2);
    assert!((evidence["last"]["x"].as_f64().unwrap() - result["x"].as_f64().unwrap()).abs() <= 1.0);
    let pointers = observed.lock().unwrap();
    assert_eq!(pointers.len(), result["steps"].as_u64().unwrap() as usize);
    assert!(
        pointers
            .iter()
            .all(|event| event.phase == "move" && event.buttons == 0)
    );
    assert_eq!(pointers.last().unwrap().x, result["x"].as_f64().unwrap());
    drop(pointers);
    // The previous center remains hittable after this 1px shift. Rect locking,
    // rather than just checking the old point, must still detect the change.
    cdp.evaluate("document.querySelector('#start').onmouseover=function(){this.style.transform='translateX(1px)'};true").unwrap();
    let origin = serde_json::from_value(result).unwrap();
    assert!(
        cdp.move_pointer("#start", &origin, Duration::from_millis(100))
            .is_err()
    );
    assert_eq!(cdp.evaluate("window.presses").unwrap(), 0);
}
