use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use url::Url;

use crate::{Error, Result};

pub const DEFAULT_CONNECT_BASE_URL: &str = "https://browser.lexmount.cn";
pub const DEFAULT_CLIENT_NAME: &str = "Agent";
pub const DEFAULT_SCOPES: &[&str] = &["browser:sessions", "browser:contexts", "browser:actions"];
const LOGIN_SUCCESS_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Lexmount authorization received</title><h1>Authorization received</h1><p>Return to your agent to confirm login completed.</p>";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials {
    pub kind: String,
    pub project_id: String,
    pub api_base_url: String,
    pub api_key: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub connect_base_url: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CredentialStatus {
    pub present: bool,
    pub valid: bool,
    pub source: &'static str,
    pub path: String,
    pub project_id_present: bool,
    pub api_key_present: bool,
    pub api_base_url: Option<String>,
    pub scopes: Vec<String>,
}

pub fn credentials_path(override_path: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = override_path {
        return Ok(path.to_path_buf());
    }
    if let Some(path) = std::env::var_os("LEXMOUNT_BROWSER_CREDENTIALS_FILE") {
        return Ok(PathBuf::from(path));
    }
    let home =
        dirs::home_dir().ok_or_else(|| Error::Config("home directory is unavailable".into()))?;
    Ok(home.join(".config/lexmount/browser-cli/credentials.json"))
}

pub fn load_credentials(path: Option<&Path>) -> Result<Option<Credentials>> {
    let path = credentials_path(path)?;
    if !path.exists() {
        return Ok(None);
    }
    let data = fs::read(&path)?;
    Ok(Some(serde_json::from_slice(&data)?))
}

pub fn status(path: Option<&Path>) -> Result<CredentialStatus> {
    let path = credentials_path(path)?;
    let env_project = std::env::var("LEXMOUNT_PROJECT_ID").ok();
    let env_key = std::env::var("LEXMOUNT_API_KEY").ok();
    if env_project.is_some() || env_key.is_some() {
        return Ok(CredentialStatus {
            present: env_project.is_some() || env_key.is_some(),
            valid: env_project.is_some() && env_key.is_some(),
            source: "environment",
            path: path.display().to_string(),
            project_id_present: env_project.is_some(),
            api_key_present: env_key.is_some(),
            api_base_url: Some(
                std::env::var("LEXMOUNT_BASE_URL")
                    .unwrap_or_else(|_| "https://api.lexmount.cn".into()),
            ),
            scopes: vec![],
        });
    }
    let credentials = load_credentials(Some(&path))?;
    Ok(match credentials {
        Some(c) => CredentialStatus {
            present: true,
            valid: c.kind == "api_key" && !c.project_id.is_empty() && !c.api_key.is_empty(),
            source: "credential_file",
            path: path.display().to_string(),
            project_id_present: !c.project_id.is_empty(),
            api_key_present: !c.api_key.is_empty(),
            api_base_url: Some(c.api_base_url),
            scopes: c.scopes,
        },
        None => CredentialStatus {
            present: false,
            valid: false,
            source: "none",
            path: path.display().to_string(),
            project_id_present: false,
            api_key_present: false,
            api_base_url: None,
            scopes: vec![],
        },
    })
}

pub fn save_credentials(credentials: &Credentials, path: Option<&Path>) -> Result<PathBuf> {
    let path = credentials_path(path)?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    // Never truncate through a symlink or expose a newly written key under the
    // process umask before chmod. Rename a private file in the same directory.
    if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(Error::Config(
            "Refusing to save credentials through a symlink".into(),
        ));
    }
    let temporary = parent.join(format!(".browser-cli-{}.tmp", random_urlsafe(16)));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> std::io::Result<()> {
        let mut file = options.open(&temporary)?;
        file.write_all(&serde_json::to_vec_pretty(credentials)?)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, &path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result?;
    Ok(path)
}

pub fn logout(path: Option<&Path>) -> Result<bool> {
    let path = credentials_path(path)?;
    if !path.exists() {
        return Ok(false);
    }
    fs::remove_file(path)?;
    Ok(true)
}

pub fn login(
    project_id: Option<&str>,
    connect_base_url: &str,
    timeout: Duration,
    open_browser: bool,
    path: Option<&Path>,
) -> Result<Value> {
    login_with_client_name(
        project_id,
        DEFAULT_CLIENT_NAME,
        connect_base_url,
        timeout,
        open_browser,
        path,
    )
}

pub fn login_with_client_name(
    project_id: Option<&str>,
    client_name: &str,
    connect_base_url: &str,
    timeout: Duration,
    open_browser: bool,
    path: Option<&Path>,
) -> Result<Value> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let redirect_uri = format!(
        "http://127.0.0.1:{}/callback",
        listener.local_addr()?.port()
    );
    let verifier = random_urlsafe(32);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state = random_urlsafe(24);
    let url = authorization_url(
        project_id,
        client_name,
        connect_base_url,
        &redirect_uri,
        &state,
        &challenge,
    )?;
    // Progress belongs on stderr so stdout remains a single command result.
    eprintln!("Open this URL in a browser on this machine to authorize:\n{url}");
    eprintln!(
        "For SSH, forward the callback port {} to this machine before opening the URL.",
        listener.local_addr()?.port()
    );
    if open_browser && let Err(error) = open::that(url.as_str()) {
        eprintln!(
            "Could not open a local browser ({error}). Open the URL manually; waiting for authorization."
        );
    }

    let started = Instant::now();
    let callback = loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream.set_read_timeout(Some(Duration::from_secs(5)))?;
                let mut buffer = [0_u8; 16_384];
                let size = stream.read(&mut buffer)?;
                let request = String::from_utf8_lossy(&buffer[..size]);
                let target = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .ok_or_else(|| Error::Config("invalid OAuth callback request".into()))?;
                let callback = Url::parse(&format!("http://127.0.0.1{target}"))
                    .map_err(|e| Error::Config(format!("invalid OAuth callback: {e}")))?;
                if callback.path() != "/callback"
                    || callback
                        .query_pairs()
                        .find(|(key, _)| key == "state")
                        .map(|(_, value)| value.into_owned())
                        .as_deref()
                        != Some(state.as_str())
                {
                    let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    if started.elapsed() >= timeout {
                        return Err(Error::Timeout("OAuth callback was not received".into()));
                    }
                    continue;
                }
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    LOGIN_SUCCESS_PAGE.len()
                )?;
                stream.write_all(LOGIN_SUCCESS_PAGE.as_bytes())?;
                break callback;
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && started.elapsed() < timeout => {
                thread::sleep(Duration::from_millis(100))
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                return Err(Error::Timeout("OAuth callback was not received. On SSH/headless Linux, open the authorization URL in your desktop browser with the callback port forwarded to this machine.".into()));
            }
            Err(e) => return Err(e.into()),
        }
    };
    let query = callback
        .query_pairs()
        .collect::<std::collections::HashMap<_, _>>();
    if query.get("state").map(|v| v.as_ref()) != Some(state.as_str()) {
        return Err(Error::Authentication("OAuth state mismatch".into()));
    }
    let code = query.get("code").ok_or_else(|| {
        Error::Authentication(
            query
                .get("error_description")
                .or_else(|| query.get("error"))
                .map(|v| v.to_string())
                .unwrap_or_else(|| "callback did not include an authorization code".into()),
        )
    })?;
    let exchange_url = format!(
        "{}/api/connect/codex/exchange",
        connect_base_url.trim_end_matches('/')
    );
    let response = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?
        .post(&exchange_url)
        .json(&json!({"code": code, "code_verifier": verifier, "redirect_uri": redirect_uri}))
        .send()?;
    let status = response.status();
    let payload: Value = response.json()?;
    if !status.is_success() {
        return Err(Error::Api {
            status: status.as_u16(),
            message: payload
                .get("message")
                .or_else(|| payload.get("error"))
                .and_then(Value::as_str)
                .unwrap_or("credential exchange failed")
                .into(),
            body: Some(payload),
        });
    }
    let mut credential = extract_api_key(&payload).ok_or_else(|| {
        Error::Authentication(
            "credential exchange response did not include project_id and api_key".into(),
        )
    })?;
    if is_internal_api_base_url(&credential.api_base_url) {
        return Err(Error::Authentication(
            "credential exchange returned an internal API base URL".into(),
        ));
    }
    credential.connect_base_url = Some(connect_base_url.trim_end_matches('/').to_owned());
    let saved = save_credentials(&credential, path)?;
    Ok(
        json!({"authenticated": true, "credentials_saved": true, "credentials_file": saved, "project_id": credential.project_id, "api_base_url": credential.api_base_url, "scopes": credential.scopes, "api_key_redacted": true}),
    )
}

fn authorization_url(
    project_id: Option<&str>,
    client_name: &str,
    connect_base_url: &str,
    redirect_uri: &str,
    state: &str,
    challenge: &str,
) -> Result<Url> {
    let scopes = DEFAULT_SCOPES.join(" ");
    let mut url = Url::parse(&format!(
        "{}/connect/codex",
        connect_base_url.trim_end_matches('/')
    ))
    .map_err(|e| Error::Config(format!("invalid connect base URL: {e}")))?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("source", "browser-cli")
            .append_pair("intent", "agent-browser-control")
            .append_pair("response", "code")
            .append_pair("expires_in", "7d")
            .append_pair("scope", &scopes)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("state", state)
            .append_pair("code_challenge", challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("client_name", client_name);
        if let Some(project_id) = project_id {
            q.append_pair("project_id", project_id);
        }
    }
    Ok(url)
}

fn random_urlsafe(size: usize) -> String {
    let mut bytes = vec![0_u8; size];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn extract_api_key(payload: &Value) -> Option<Credentials> {
    let mut candidates = vec![payload];
    for key in ["credential", "credentials", "token", "env"] {
        if let Some(value) = payload.get(key) {
            candidates.push(value);
        }
    }
    for value in candidates {
        let project_id = value
            .get("project_id")
            .or_else(|| value.get("projectId"))
            .or_else(|| value.get("LEXMOUNT_PROJECT_ID"))
            .or_else(|| payload.get("project_id"))
            .and_then(Value::as_str);
        let api_key = value
            .get("api_key")
            .or_else(|| value.get("apiKey"))
            .or_else(|| value.get("LEXMOUNT_API_KEY"))
            .or_else(|| payload.get("api_key"))
            .and_then(Value::as_str);
        if let (Some(project_id), Some(api_key)) = (project_id, api_key) {
            if project_id.trim().is_empty() || api_key.trim().is_empty() {
                continue;
            }
            let scopes = value
                .get("scopes")
                .or_else(|| value.get("scope"))
                .or_else(|| payload.get("scopes"));
            let scopes = match scopes {
                Some(Value::Array(v)) => v
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
                Some(Value::String(v)) => v.split_whitespace().map(str::to_owned).collect(),
                _ => DEFAULT_SCOPES.iter().map(|v| (*v).to_owned()).collect(),
            };
            return Some(Credentials {
                kind: "api_key".into(),
                project_id: project_id.into(),
                api_base_url: value
                    .get("api_base_url")
                    .or_else(|| value.get("apiBaseUrl"))
                    .or_else(|| payload.get("api_base_url"))
                    .and_then(Value::as_str)
                    .unwrap_or("https://api.lexmount.cn")
                    .trim_end_matches('/')
                    .into(),
                api_key: api_key.into(),
                scopes,
                expires_at: value
                    .get("expires_at")
                    .or_else(|| value.get("expiresAt"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                source: Some("connect_from_browser_cli".into()),
                connect_base_url: Some(DEFAULT_CONNECT_BASE_URL.into()),
                created_at: None,
            });
        }
    }
    None
}

fn is_internal_api_base_url(value: &str) -> bool {
    let host = Url::parse(value)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| value.split('/').next().unwrap_or(value).to_owned())
        .trim_end_matches('.')
        .to_ascii_lowercase();
    host.contains(".svc.") || host.ends_with(".svc") || host.ends_with(".cluster.local")
}

/// Device authorization needs only outbound HTTPS; no loopback listener or opener.
pub fn device_login(
    project_id: Option<&str>,
    client_name: &str,
    connect_base_url: &str,
    timeout: Duration,
    path: Option<&Path>,
) -> Result<Value> {
    let base = Url::parse(connect_base_url).map_err(|e| Error::Config(e.to_string()))?;
    let loopback = matches!(base.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    if base.scheme() != "https" && !(base.scheme() == "http" && loopback) {
        return Err(Error::Config("device login requires HTTPS".into()));
    }
    let started = Instant::now();
    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()?;
    let response = client
        .post(base.join("/api/connect/device/authorize").unwrap())
        .json(&json!({"client_name":client_name}))
        .send()?;
    if !response.status().is_success() {
        return Err(Error::Authentication(format!(
            "Device login could not start (HTTP {}). The console must support device authorization.",
            response.status()
        )));
    }
    let payload: Value = response.json()?;
    let secret = payload["device_code"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::Authentication("Missing device code".into()))?;
    let code = payload["user_code"]
        .as_str()
        .ok_or_else(|| Error::Authentication("Missing user code".into()))?;
    let verify = base
        .join(
            payload["verification_uri"]
                .as_str()
                .unwrap_or("/connect/device"),
        )
        .map_err(|e| Error::Config(e.to_string()))?;
    if verify.origin() != base.origin() {
        return Err(Error::Authentication(
            "Verification page must use the selected console origin".into(),
        ));
    }
    let lifetime = timeout.min(Duration::from_secs(
        payload["expires_in"].as_u64().unwrap_or(600),
    ));
    let mut interval = payload["interval"].as_u64().unwrap_or(5).max(1);
    eprintln!(
        "Open on your phone or another computer: {verify}\nEnter code: {code}\nWaiting for approval ({} seconds). Ctrl+C to cancel.",
        lifetime.as_secs()
    );
    loop {
        let remaining = lifetime.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(Error::Timeout(
                "Device authorization expired; run auth login again".into(),
            ));
        }
        thread::sleep(Duration::from_secs(interval).min(remaining));
        let remaining = lifetime.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            continue;
        }
        let response = match client
            .post(base.join("/api/connect/device/token").unwrap())
            .timeout(remaining.min(Duration::from_secs(15)))
            .json(&json!({"device_code":secret}))
            .send()
        {
            Ok(response) => response,
            Err(error) if error.is_timeout() || error.is_connect() => {
                interval = interval.saturating_mul(2).min(30);
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        let status = response.status();
        if status.is_server_error() {
            interval = interval.saturating_mul(2).min(30);
            continue;
        }
        let payload: Value = response.json()?;
        if !status.is_success() {
            match payload["error"].as_str() {
                Some("authorization_pending") => continue,
                Some("slow_down") => {
                    interval = interval.saturating_add(5);
                    continue;
                }
                Some("access_denied") => {
                    return Err(Error::Authentication("Device authorization denied".into()));
                }
                Some("expired_token") => {
                    return Err(Error::Authentication(
                        "Device authorization expired; run auth login again".into(),
                    ));
                }
                _ => {
                    return Err(Error::Authentication(
                        "Device authorization failed; run auth login again".into(),
                    ));
                }
            }
        }
        let mut credential = extract_api_key(&payload)
            .ok_or_else(|| Error::Authentication("Invalid device credential response".into()))?;
        if project_id.is_some_and(|id| id != credential.project_id) {
            return Err(Error::Authentication(
                "Authorized project does not match --project-id; credentials were not saved".into(),
            ));
        }
        if is_internal_api_base_url(&credential.api_base_url)
            || Url::parse(&credential.api_base_url).map_or(true, |u| {
                u.scheme() != "https"
                    || u.host_str().is_none()
                    || !u.username().is_empty()
                    || u.password().is_some()
            })
        {
            return Err(Error::Authentication("Invalid device API base URL".into()));
        }
        credential.connect_base_url = Some(connect_base_url.trim_end_matches('/').to_owned());
        let saved = save_credentials(&credential, path)?;
        return Ok(
            json!({"authenticated":true,"credentials_saved":true,"credentials_file":saved,
            "project_id":credential.project_id,"api_base_url":credential.api_base_url,"api_key_redacted":true}),
        );
    }
}

pub fn prefers_device_login() -> bool {
    std::env::var_os("SSH_CONNECTION").is_some()
        || std::env::var_os("SSH_TTY").is_some()
        || (cfg!(target_os = "linux")
            && std::env::var_os("DISPLAY").is_none()
            && std::env::var_os("WAYLAND_DISPLAY").is_none())
}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(unix)]
    fn credential_save_rejects_symlink_and_replaces_file_privately() {
        use std::os::unix::{fs::PermissionsExt, fs::symlink};
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        std::fs::write(&target, "original").unwrap();
        let link = dir.path().join("credentials");
        symlink(&target, &link).unwrap();
        let credentials =
            super::extract_api_key(&serde_json::json!({"project_id":"p","api_key":"secret"}))
                .unwrap();
        assert!(super::save_credentials(&credentials, Some(&link)).is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "original");
        super::save_credentials(&credentials, Some(&target)).unwrap();
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }
    #[test]
    fn empty_credentials_are_rejected() {
        assert!(
            super::extract_api_key(&serde_json::json!({"project_id":"", "api_key":""})).is_none()
        );
    }

    use super::*;

    #[test]
    fn extracts_nested_credentials_without_leaking_secret() {
        let c = extract_api_key(&json!({"credential":{"projectId":"p1","apiKey":"secret","apiBaseUrl":"https://api.example","scope":"browser:sessions browser:actions"}})).unwrap();
        assert_eq!(c.project_id, "p1");
        assert_eq!(c.api_key, "secret");
        assert_eq!(c.scopes.len(), 2);
        assert_eq!(c.source.as_deref(), Some("connect_from_browser_cli"));
    }

    #[test]
    fn authorization_url_encodes_custom_client_name() {
        let client_name = "Claude Desktop 中文";
        let url = authorization_url(
            Some("project-1"),
            client_name,
            "https://browser.example/",
            "http://127.0.0.1:1234/callback",
            "state",
            "challenge",
        )
        .unwrap();

        assert!(
            url.as_str()
                .contains("client_name=Claude+Desktop+%E4%B8%AD%E6%96%87")
        );
        assert_eq!(
            url.query_pairs()
                .find(|(key, _)| key.as_ref() == "client_name")
                .map(|(_, value)| value.into_owned())
                .as_deref(),
            Some(client_name)
        );
    }

    #[test]
    fn login_success_page_is_agent_agnostic() {
        assert!(
            !LOGIN_SUCCESS_PAGE
                .to_ascii_lowercase()
                .contains("workbuddy")
        );
        assert!(LOGIN_SUCCESS_PAGE.contains("Return to your agent"));
    }

    #[test]
    fn legacy_login_api_signature_is_preserved() {
        type LegacyLogin = for<'a, 'b, 'c> fn(
            Option<&'a str>,
            &'b str,
            Duration,
            bool,
            Option<&'c Path>,
        ) -> Result<Value>;
        let _: LegacyLogin = login;
    }

    #[test]
    fn rejects_internal_cluster_api_hosts() {
        assert!(is_internal_api_base_url(
            "http://browser-api.default.svc.cluster.local"
        ));
        assert!(!is_internal_api_base_url("https://api.lexmount.cn"));
    }
}
