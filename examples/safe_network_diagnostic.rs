//! Development-only read-only diagnostic. Never emits URLs, credentials,
//! response bodies or error messages; the regular CLI contract is unchanged.
use std::error::Error as StdError;

use lexmount_browser::{Client, Error};
use serde_json::{Value, json};

fn main() {
    let outcome = Client::from_env().and_then(|client| client.list_sessions(Some("active")));
    let report = match outcome {
        Ok(_) => json!({"ok":true,"operation":"list_active_sessions","network_reached_api":true}),
        Err(Error::Http(error)) => {
            let mut io_errors = Vec::new();
            let mut categories = Vec::new();
            let mut security_codes = Vec::new();
            let mut current: Option<&(dyn StdError + 'static)> = Some(&error);
            for _ in 0..16 {
                let Some(source) = current else {
                    break;
                };
                if let Some(io_error) = source.downcast_ref::<std::io::Error>() {
                    io_errors.push(json!({"kind":format!("{:?}",io_error.kind()),"os_error":io_error.raw_os_error()}));
                }
                // Inspect locally for allowlisted classifications only. Never
                // serialize the source message, Debug output or a source URL.
                let message = source.to_string().to_lowercase();
                for (category, words) in [
                    (
                        "permission",
                        &["permission denied", "operation not permitted"][..],
                    ),
                    ("dns", &["dns", "failed to lookup", "could not resolve"][..]),
                    (
                        "certificate",
                        &[
                            "invalid peer certificate",
                            "certificate verify",
                            "unknownissuer",
                            "unknown issuer",
                        ][..],
                    ),
                    (
                        "trust_evaluation",
                        &["trust evaluation", "security framework", "keychain"][..],
                    ),
                    ("connection_refused", &["connection refused"][..]),
                    ("connection_reset", &["connection reset"][..]),
                    (
                        "network_unreachable",
                        &["network is unreachable", "no route to host"][..],
                    ),
                    ("timeout", &["timed out", "timeout"][..]),
                    ("proxy", &["proxy"][..]),
                ] {
                    if words.iter().any(|word| message.contains(word))
                        && !categories.contains(&category)
                    {
                        categories.push(category);
                    }
                }
                for code in [-34018, -25291, -25300, -25293, -67843] {
                    if message.contains(&code.to_string()) && !security_codes.contains(&code) {
                        security_codes.push(code);
                    }
                }
                current = source.source();
            }
            json!({"ok":false,"error_kind":"http_error","is_connect":error.is_connect(),
                "is_timeout":error.is_timeout(),"is_request":error.is_request(),
                "http_status":error.status().map(|status|status.as_u16()),
                "source_categories":categories,"io_errors":io_errors,"security_codes":security_codes})
        }
        Err(error) => {
            let (kind, status): (&str, Value) = match error {
                Error::Config(_) => ("configuration_error", Value::Null),
                Error::Authentication(_) => ("authentication_error", Value::Null),
                Error::Api { status, .. } => ("api_error", json!(status)),
                Error::Timeout(_) => ("timeout", Value::Null),
                Error::Io(error) => {
                    println!(
                        "{}",
                        json!({"ok":false,"error_kind":"io_error","io_kind":format!("{:?}",error.kind()),"os_error":error.raw_os_error()})
                    );
                    return;
                }
                _ => ("other_error", Value::Null),
            };
            json!({"ok":false,"error_kind":kind,"http_status":status})
        }
    };
    println!("{report}");
}
