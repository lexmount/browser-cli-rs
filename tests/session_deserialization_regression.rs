use lexmount_browser::{Client, Session};

#[test]
fn session_accepts_null_for_nullable_string_field() {
    let payload = r#"{
        "session_id":"session-test",
        "status":"active",
        "project_id":"project-test",
        "browser_mode":"normal",
        "inspect_url":null
    }"#;

    let session = serde_json::from_str::<Session>(payload).unwrap();
    assert_eq!(session.session_id, "session-test");
    assert_eq!(session.inspect_url, "");
}

#[test]
fn session_accepts_id_and_session_id_and_prefers_canonical_values() {
    let payload = r#"{
        "id":"legacy-session-id",
        "session_id":"canonical-session-id",
        "status":"active",
        "project_id":"project-test",
        "browser_mode":"normal",
        "browser_type":"light",
        "inspect_url":""
    }"#;

    let session = serde_json::from_str::<Session>(payload).unwrap();
    assert_eq!(session.session_id, "canonical-session-id");
    assert_eq!(session.browser_mode, "normal");
}

#[test]
fn session_accepts_legacy_id_when_session_id_is_absent() {
    let payload = r#"{"id":"legacy-session-id","inspect_url":null}"#;
    let session = serde_json::from_str::<Session>(payload).unwrap();
    assert_eq!(session.session_id, "legacy-session-id");
}

#[test]
fn session_rejects_a_response_without_any_session_id() {
    let error = serde_json::from_str::<Session>(r#"{"status":"active"}"#).unwrap_err();
    assert!(error.to_string().contains("session_id or id"));
}

#[test]
#[ignore = "requires configured Lexmount credentials and live API access"]
fn live_session_list_accepts_current_api_response() {
    let client = Client::from_env().expect("configured Lexmount credentials");
    let sessions = client
        .list_sessions(Some("active"))
        .expect("current API response should deserialize");
    assert!(
        sessions
            .sessions
            .iter()
            .all(|session| !session.session_id.is_empty())
    );
}
