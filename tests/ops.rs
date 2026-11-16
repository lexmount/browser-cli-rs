use std::time::Duration;

use httpmock::{
    Method::{GET, POST},
    MockServer,
};
use lexmount_browser::{
    Client,
    ops::{CreateLogin, LoginTask},
};
use serde_json::json;

const SITE: &str = "https://example.test";

fn client(server: &MockServer) -> Client {
    Client::builder()
        .api_key("fixture-api-key")
        .project_id("project")
        .base_url(server.base_url())
        .region("test-region")
        .build()
        .unwrap()
}

#[test]
fn ops_session_uses_requested_image_and_isolation_compatible_options() {
    let server = MockServer::start();
    let create = server.mock(|when, then| {
        when.method(POST).path("/instance/v2").json_body(json!({
            "browser_mode":"normal", "custom_image_id":"registry.example.test/browser:test",
            "enable_lightmount_layout":false, "enable_lightmount_resource":false,
            "enable_captcha":false, "downloads":{"enabled":false},
            "recording":{"persistent":false}, "region_id":"test-region",
        }));
        then.status(202)
            .json_body(json!({"session_id":"session-1"}));
    });
    let poll = server.mock(|when, then| {
        when.method(POST).path("/instance/session");
        then.json_body(json!({"session_id":"session-1","status":"active"}));
    });
    let session = client(&server)
        .create_ops_session("registry.example.test/browser:test", Duration::from_secs(3))
        .unwrap();
    assert_eq!(session.session_id, "session-1");
    create.assert_hits(1);
    poll.assert_hits(1);
}

#[test]
fn ops_selects_unique_item_across_pages_and_sends_deployed_request() {
    let server = MockServer::start();
    let connection = server.mock(|when, then| {
        when.method(GET)
            .path("/v1/credential-connections/conn")
            .header("x-api-key", "fixture-api-key")
            .header("x-project-id", "project");
        then.json_body(json!({"id":"conn","name":"connection-name"}));
    });
    let make_page = |items, next_cursor| {
        json!({
            "items":items,"nextCursor":next_cursor,"connectionId":"conn",
            "enumerationComplete":true,"siteMatchComplete":true,"requiresExplicitSelection":false,
        })
    };
    server.mock(|when, then| {
        when.method(GET)
            .path("/v1/credential-connections/conn/items")
            .query_param("siteOrigin", SITE)
            .matches(|request| {
                request
                    .query_params
                    .as_ref()
                    .is_none_or(|query| !query.iter().any(|(key, _)| key == "cursor"))
            });
        then.json_body(make_page(json!([]), json!("opaque+/=")));
    });
    let next = server.mock(|when, then| {
        when.method(GET)
            .path("/v1/credential-connections/conn/items")
            .query_param("siteOrigin", SITE)
            .query_param("cursor", "opaque+/=");
        then.json_body(make_page(
            json!([{"vaultId":"vault","itemId":"item"}]),
            json!(null),
        ));
    });
    let mut create = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/login-tasks")
            .header_exists("Idempotency-Key")
            .json_body(json!({
                "sessionId":"session-1", "name":"connection-name", "url":SITE,
                "vaultId":"vault","itemId":"item",
            }));
        then.status(202).json_body(
            json!({"data":{"taskId":"task-1","sessionId":"session-1","state":"queued"}}),
        );
    });
    let task = client(&server)
        .create_login_task(CreateLogin {
            session_id: "session-1",
            connection_id: "conn",
            site_origin: SITE,
            item: None,
            idempotency_key: None,
        })
        .unwrap();
    assert_eq!(task.id, "task-1");
    connection.assert_hits(1);
    next.assert_hits(1);
    create.assert_hits(1);
    create.delete();

    let unavailable = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/login-tasks")
            .header("Idempotency-Key", "same-intent")
            .json_body(json!({
                "sessionId":"session-1", "name":"connection-name", "url":SITE,
                "vaultId":"vault", "itemId":"item",
            }));
        then.status(504)
            .header("Retry-After", "0")
            .body("fixture-api-key");
    });
    let error = client(&server)
        .create_login_task(CreateLogin {
            session_id: "session-1",
            connection_id: "conn",
            site_origin: SITE,
            item: Some(lexmount_browser::ops::LoginItemRef {
                vault_id: "vault".into(),
                item_id: "item".into(),
            }),
            idempotency_key: Some("same-intent"),
        })
        .unwrap_err();
    assert!(error.to_string().contains("504"));
    assert!(!format!("{error:?}").contains("fixture-api-key"));
    unavailable.assert_hits(3);
}

#[test]
fn ops_multiple_accounts_require_selection_without_submitting() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/v1/credential-connections/conn");
        then.json_body(json!({"id":"conn","name":"connection-name"}));
    });
    server.mock(|when, then| {
        when.method(GET)
            .path("/v1/credential-connections/conn/items");
        then.json_body(json!({
            "connectionId":"conn", "enumerationComplete":true, "siteMatchComplete":true,
            "requiresExplicitSelection":true, "nextCursor":null,
            "items":[{"vaultId":"v","itemId":"i"},{"vaultId":"v","itemId":"j"}],
        }));
    });
    let create = server.mock(|when, then| {
        when.method(POST).path("/v1/login-tasks");
        then.status(500);
    });
    let error = client(&server)
        .create_login_task(CreateLogin {
            session_id: "session-1",
            connection_id: "conn",
            site_origin: SITE,
            item: None,
            idempotency_key: None,
        })
        .unwrap_err();
    assert!(error.to_string().contains("select an OPS account"));
    create.assert_hits(0);
}

#[test]
fn ops_wait_returns_manual_verification_without_exporting_open_url() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/v1/login-tasks/task-1");
        then.json_body(json!({
            "id":"task-1","state":"awaiting_user","requiresAction":{
                "type":"manual_verification","openUrl":"https://example.test/secret-url"
            },
        }));
    });
    let task: LoginTask = client(&server)
        .wait_for_login_task("task-1", Duration::from_secs(3))
        .unwrap();
    assert_eq!(task.state, "awaiting_user");
    assert!(!serde_json::to_string(&task).unwrap().contains("secret-url"));
    assert!(!format!("{task:?}").contains("secret-url"));
}
