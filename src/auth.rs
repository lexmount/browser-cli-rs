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
const LOGIN_SUCCESS_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Lexmount connected</title><h1>Lexmount connected</h1><p>You can close this window and return to your agent.</p>";

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
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, serde_json::to_vec_pretty(credentials)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    }
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
    if open_browser {
        open::that(url.as_str()).map_err(|e| Error::Io(std::io::Error::other(e)))?;
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
                return Err(Error::Timeout("OAuth callback was not received".into()));
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

#[cfg(test)]
mod tests {
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
        assert!(LOGIN_SUCCESS_PAGE.contains("return to your agent"));
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
