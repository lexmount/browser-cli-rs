//! Exercise the real CLI and existing v2 wire fields against local fixtures only.
mod support;

use httpmock::{Method::POST, MockServer};
use serde_json::json;

#[test]
fn cli_reads_each_requested_page_without_changing_the_json_envelope() {
    let server = MockServer::start();
    let directory = tempfile::tempdir().unwrap();
    let mut requests = Vec::new();
    for page in [1, 2] {
        requests.push(server.mock(|when, then| {
            when.method(POST).path("/instance/v2/sessions")
                .json_body(json!({"status":"active","page":page,"pageSize":2}));
            then.status(200).json_body(json!({
                "sessions": if page == 1 {json!([
                    {"session_id":"first","status":"active"},{"session_id":"second","status":"active"}
                ])} else {json!([{"session_id":"third","status":"active"}])},
                "pagination":{"currentPage":page,"pageSize":2,"totalCount":3,"totalPages":2,"activeCount":3,"closedCount":0}
            }));
        }));
    }
    let mut ids = Vec::new();
    for page in ["1", "2"] {
        let data = support::data(support::cli(
            &server.base_url(),
            directory.path(),
            &[
                "session",
                "list",
                "--status",
                "active",
                "--page",
                page,
                "--page-size",
                "2",
            ],
        ));
        assert_eq!(data.as_object().unwrap().len(), 2);
        assert_eq!(
            data["pagination"]["currentPage"],
            page.parse::<u64>().unwrap()
        );
        assert_eq!(data["pagination"]["pageSize"], 2);
        assert_eq!(data["pagination"]["totalPages"], 2);
        ids.extend(
            data["sessions"]
                .as_array()
                .unwrap()
                .iter()
                .map(|session| session["session_id"].clone()),
        );
    }
    assert_eq!(ids, vec![json!("first"), json!("second"), json!("third")]);
    for request in requests {
        request.assert_hits(1);
    }
}

#[test]
fn optional_page_flags_do_not_inject_defaults_into_legacy_requests() {
    for (arguments, body) in [
        (
            vec!["session", "list", "--status", "active"],
            json!({"status":"active"}),
        ),
        (vec!["session", "list", "--page", "2"], json!({"page":2})),
        (
            vec!["session", "list", "--page-size", "25"],
            json!({"pageSize":25}),
        ),
    ] {
        let server = MockServer::start();
        let directory = tempfile::tempdir().unwrap();
        let request = server.mock(|when, then| {
            when.method(POST).path("/instance/v2/sessions").json_body(body);
            then.status(200).json_body(json!({"sessions":[],"pagination":{"currentPage":1,"pageSize":5,"totalCount":0,"totalPages":0}}));
        });
        let data = support::data(support::cli(
            &server.base_url(),
            directory.path(),
            &arguments,
        ));
        assert_eq!(data["sessions"], json!([]));
        assert_eq!(data["pagination"]["pageSize"], 5);
        request.assert_hits(1);
    }
}

#[test]
fn invalid_page_arguments_are_rejected_before_network_access() {
    let directory = tempfile::tempdir().unwrap();
    for (flag, value) in [
        ("--page", "0"),
        ("--page-size", "0"),
        ("--page", "-1"),
        ("--page-size", "not-a-number"),
    ] {
        let output = support::cli(
            "http://127.0.0.1:1",
            directory.path(),
            &["session", "list", flag, value],
        );
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    }
}
