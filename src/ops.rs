use std::{
    collections::HashSet,
    thread,
    time::{Duration, Instant},
};

use reqwest::{Method, Url};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::{Client, Error, Result, Session};

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialConnection {
    pub id: String,
    pub name: Option<String>,
    pub label: Option<String>,
    pub provider: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionPage {
    pub items: Vec<CredentialConnection>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginItemRef {
    pub vault_id: String,
    pub item_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginItem {
    #[serde(flatten)]
    pub reference: LoginItemRef,
    pub title: Option<String>,
    pub username_hint: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginItemPage {
    pub items: Vec<LoginItem>,
    pub next_cursor: Option<String>,
    pub connection_id: String,
    pub enumeration_complete: bool,
    pub site_match_complete: bool,
    pub requires_explicit_selection: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginTaskError {
    pub code: Option<String>,
    pub phase: Option<String>,
    #[serde(default)]
    pub retryable: bool,
    pub trace_id: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginAction {
    #[serde(rename = "type")]
    pub kind: String,
    pub expires_at: Option<String>,
    #[serde(skip_serializing)]
    pub open_url: Option<String>,
}

impl std::fmt::Debug for LoginAction {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LoginAction")
            .field("kind", &self.kind)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginTask {
    #[serde(alias = "taskId")]
    pub id: String,
    pub state: String,
    pub phase: Option<String>,
    pub session_id: Option<String>,
    #[serde(default)]
    pub cancel_requested: bool,
    pub requires_action: Option<LoginAction>,
    pub error: Option<LoginTaskError>,
}

pub struct CreateLogin<'a> {
    pub session_id: &'a str,
    pub connection_id: &'a str,
    pub site_origin: &'a str,
    pub item: Option<LoginItemRef>,
    pub idempotency_key: Option<&'a str>,
}

fn invalid_response() -> Error {
    Error::Config("invalid OPS API response".into())
}

fn identifier(value: &str, field: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 256
        || matches!(value, "." | "..")
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.:-".contains(&c))
    {
        return Err(Error::Config(format!("invalid {field}")));
    }
    Ok(())
}

fn https_origin(value: &str) -> Result<String> {
    let url =
        Url::parse(value).map_err(|_| Error::Config("expected an HTTPS site origin".into()))?;
    if url.scheme() != "https"
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || value.contains(['@', '\\'])
        || value.trim() != value
    {
        return Err(Error::Config("expected an HTTPS site origin".into()));
    }
    Ok(url.origin().ascii_serialization())
}

fn page_query(cursor: Option<&str>, limit: u8) -> Result<Vec<(&'static str, String)>> {
    if !(1..=100).contains(&limit) {
        return Err(Error::Config(
            "OPS page limit must be between 1 and 100".into(),
        ));
    }
    let mut query = vec![("limit", limit.to_string())];
    if let Some(cursor) = cursor {
        if cursor.is_empty() || cursor.chars().any(char::is_control) {
            return Err(Error::Config("invalid OPS page cursor".into()));
        }
        query.push(("cursor", cursor.to_owned()));
    }
    Ok(query)
}

impl Client {
    fn ops_request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<Value>,
        key: Option<&str>,
        timeout: Option<Duration>,
    ) -> Result<T> {
        let body_is_read = method == Method::GET;
        let mut request = self
            .ops_http
            .request(method, format!("{}{path}", self.base_url()))
            .header("x-api-key", &self.api_key)
            .header("x-project-id", self.project_id())
            .header("accept", "application/json")
            .query(query);
        if let Some(body) = body {
            request = request.json(&body);
        }
        if let Some(key) = key {
            request = request.header("Idempotency-Key", key);
        }
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        // Only reads and starts with a stable idempotency key are safe to retry.
        let retries = if timeout.is_none() && (key.is_some() || body_is_read) {
            2
        } else {
            0
        };
        let mut retries_left = retries;
        let response = loop {
            if retries_left == 0 {
                break request
                    .send()
                    .map_err(|error| Error::Http(error.without_url()))?;
            }
            match request.try_clone().ok_or_else(invalid_response)?.send() {
                Ok(response)
                    if retries_left > 0
                        && matches!(response.status().as_u16(), 429 | 502 | 503 | 504) =>
                {
                    let retry_after = response.headers().get("retry-after");
                    let delay = retry_after
                        .map_or(Some(1), |header| header.to_str().ok()?.parse::<u64>().ok());
                    let Some(delay @ 0..=5) = delay else {
                        break response;
                    };
                    thread::sleep(Duration::from_secs(delay));
                }
                Err(ref error)
                    if retries_left > 0 && (error.is_connect() || error.is_timeout()) =>
                {
                    thread::sleep(Duration::from_secs(1));
                }
                Ok(response) => break response,
                Err(error) => return Err(Error::Http(error.without_url())),
            }
            retries_left -= 1;
        };
        let status = response.status();
        if !status.is_success() {
            // Credential endpoints can return sensitive diagnostics in error bodies.
            return Err(Error::Api {
                status: status.as_u16(),
                message: "OPS request rejected".into(),
                body: None,
            });
        }
        let mut value: Value = response.json().map_err(|_| invalid_response())?;
        redact(&mut value, &self.api_key);
        if let Some(data) = value.get_mut("data") {
            return serde_json::from_value(data.take()).map_err(|_| invalid_response());
        }
        serde_json::from_value(value).map_err(|_| invalid_response())
    }

    pub fn create_ops_session(&self, image: &str, timeout: Duration) -> Result<Session> {
        if image.trim().is_empty() || image.chars().any(char::is_whitespace) {
            return Err(Error::Config(
                "an explicit Chrome image is required for an OPS session".into(),
            ));
        }
        let body = json!({
            "browser_mode": "normal", "custom_image_id": image,
            "enable_lightmount_layout": false, "enable_lightmount_resource": false,
            "enable_captcha": false, "downloads": {"enabled": false},
            "recording": {"persistent": false}
        });
        self.create_session_from_body(self.body_with_region(body), timeout, false)
    }

    pub fn list_credential_connections(
        &self,
        cursor: Option<&str>,
        limit: u8,
    ) -> Result<ConnectionPage> {
        self.ops_request(
            Method::GET,
            "/v1/credential-connections",
            &page_query(cursor, limit)?,
            None,
            None,
            None,
        )
    }

    pub fn list_login_items(
        &self,
        connection_id: &str,
        site_origin: &str,
        cursor: Option<&str>,
        limit: u8,
    ) -> Result<LoginItemPage> {
        identifier(connection_id, "connection ID")?;
        let mut query = page_query(cursor, limit)?;
        query.push(("siteOrigin", https_origin(site_origin)?));
        let page: LoginItemPage = self.ops_request(
            Method::GET,
            &format!("/v1/credential-connections/{connection_id}/items"),
            &query,
            None,
            None,
            None,
        )?;
        if page.connection_id != connection_id {
            return Err(invalid_response());
        }
        Ok(page)
    }

    pub fn create_login_task(&self, options: CreateLogin<'_>) -> Result<LoginTask> {
        identifier(options.session_id, "session ID")?;
        identifier(options.connection_id, "connection ID")?;
        let origin = https_origin(options.site_origin)?;
        if let Some(item) = &options.item {
            identifier(&item.vault_id, "vault ID")?;
            identifier(&item.item_id, "item ID")?;
        }
        if options.idempotency_key.is_some() && options.item.is_none() {
            return Err(Error::Config(
                "an explicit OPS item is required when reusing an idempotency key".into(),
            ));
        }
        let generated_key = format!("{:032x}", rand::random::<u128>());
        let key = options.idempotency_key.unwrap_or(&generated_key);
        if key.trim().is_empty() || key.len() > 256 || !key.bytes().all(|c| (32..127).contains(&c))
        {
            return Err(Error::Config("invalid idempotency key".into()));
        }
        let connection: CredentialConnection = self.ops_request(
            Method::GET,
            &format!("/v1/credential-connections/{}", options.connection_id),
            &[],
            None,
            None,
            None,
        )?;
        if connection.id != options.connection_id {
            return Err(invalid_response());
        }
        let name = connection
            .name
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(invalid_response)?;
        let item = self.select_login_item(options.connection_id, &origin, options.item.as_ref())?;
        let task: LoginTask = self.ops_request(
            Method::POST,
            "/v1/login-tasks",
            &[],
            Some(json!({
                "sessionId": options.session_id, "name": name, "url": origin,
                "vaultId": item.vault_id, "itemId": item.item_id,
            })),
            Some(key),
            None,
        )?;
        if task.session_id.as_deref() != Some(options.session_id) {
            return Err(invalid_response());
        }
        identifier(&task.id, "returned login task ID")?;
        Ok(task)
    }

    fn select_login_item(
        &self,
        connection: &str,
        origin: &str,
        explicit: Option<&LoginItemRef>,
    ) -> Result<LoginItemRef> {
        let mut cursor = None;
        let mut seen_cursors = HashSet::new();
        let mut items = HashSet::new();
        let mut requires_explicit = false;
        for _ in 0..20 {
            let page = self.list_login_items(connection, origin, cursor.as_deref(), 50)?;
            if !page.enumeration_complete || !page.site_match_complete {
                break;
            }
            requires_explicit |= page.requires_explicit_selection;
            for item in page.items {
                identifier(&item.reference.vault_id, "returned vault ID")?;
                identifier(&item.reference.item_id, "returned item ID")?;
                items.insert(item.reference);
            }
            cursor = page.next_cursor;
            match &cursor {
                None => {
                    if let Some(explicit) = explicit {
                        return items.take(explicit).ok_or_else(|| {
                            Error::NotFound("selected OPS item does not match the site".into())
                        });
                    }
                    if requires_explicit || items.len() > 1 {
                        return Err(Error::Config(
                            "select an OPS account with --vault-id and --item-id".into(),
                        ));
                    }
                    return items
                        .into_iter()
                        .next()
                        .ok_or_else(|| Error::NotFound("no OPS item matches the site".into()));
                }
                Some(value) if !seen_cursors.insert(value.clone()) => break,
                Some(_) => {}
            }
        }
        Err(Error::Config("OPS item listing was incomplete".into()))
    }

    fn request_login_task(
        &self,
        task_id: &str,
        action: Option<&str>,
        timeout: Option<Duration>,
    ) -> Result<LoginTask> {
        identifier(task_id, "login task ID")?;
        let mut path = format!("/v1/login-tasks/{task_id}");
        if let Some(action) = action {
            path.push('/');
            path.push_str(action);
        }
        let task: LoginTask = self.ops_request(
            if action.is_some() {
                Method::POST
            } else {
                Method::GET
            },
            &path,
            &[],
            action.map(|_| json!({})),
            None,
            timeout,
        )?;
        if task.id != task_id {
            return Err(invalid_response());
        }
        Ok(task)
    }

    pub fn get_login_task(&self, task_id: &str) -> Result<LoginTask> {
        self.request_login_task(task_id, None, None)
    }

    pub fn resume_login_task(&self, task_id: &str) -> Result<LoginTask> {
        self.request_login_task(task_id, Some("resume"), None)
    }

    pub fn cancel_login_task(&self, task_id: &str) -> Result<LoginTask> {
        self.request_login_task(task_id, Some("cancel"), None)
    }

    pub fn wait_for_login_task(&self, task_id: &str, timeout: Duration) -> Result<LoginTask> {
        identifier(task_id, "login task ID")?;
        let started = Instant::now();
        loop {
            let remaining = timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return Err(Error::Timeout(format!("waiting for OPS task {task_id}")));
            }
            let task = self.request_login_task(
                task_id,
                None,
                Some(remaining.min(Duration::from_secs(10))),
            )?;
            match task.state.as_str() {
                "succeeded" | "failed" | "cancelled" | "expired" => return Ok(task),
                "awaiting_user" if !task.cancel_requested => return Ok(task),
                "queued" | "running" | "awaiting_user" | "finalizing" => {}
                _ => return Err(invalid_response()),
            }
            thread::sleep(Duration::from_secs(1).min(timeout.saturating_sub(started.elapsed())));
        }
    }
}

fn redact(value: &mut Value, secret: &str) {
    if secret.is_empty() {
        return;
    }
    match value {
        Value::String(value) if value.contains(secret) => {
            *value = value.replace(secret, "[REDACTED]")
        }
        Value::Array(items) => items.iter_mut().for_each(|item| redact(item, secret)),
        Value::Object(fields) => fields.values_mut().for_each(|item| redact(item, secret)),
        _ => {}
    }
}
