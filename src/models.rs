use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Serialize, Default)]
pub struct Session {
    pub session_id: String,
    pub status: String,
    pub project_id: String,
    pub browser_mode: String,
    pub region_id: Option<String>,
    pub context_id: Option<String>,
    pub context_description: Option<String>,
    pub context_display_name: Option<String>,
    pub created_at: Value,
    pub inspect_url: String,
    pub container_id: Option<String>,
    pub ws: Option<String>,
    pub create_error: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl<'de> Deserialize<'de> for Session {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // The API has returned both `id` and `session_id` in one object, and
        // nullable values for fields that were historically strings. Accept
        // both response shapes without changing the public model.
        #[derive(Deserialize, Default)]
        struct SessionResponse {
            #[serde(default)]
            session_id: Option<String>,
            #[serde(default)]
            id: Option<String>,
            #[serde(default)]
            status: Option<String>,
            #[serde(default)]
            project_id: Option<String>,
            #[serde(default)]
            browser_mode: Option<String>,
            #[serde(default)]
            browser_type: Option<String>,
            #[serde(default)]
            region_id: Option<String>,
            #[serde(default)]
            context_id: Option<String>,
            #[serde(default)]
            context_description: Option<String>,
            #[serde(default)]
            context_display_name: Option<String>,
            #[serde(default)]
            created_at: Value,
            #[serde(default)]
            inspect_url: Option<String>,
            #[serde(default)]
            container_id: Option<String>,
            #[serde(default)]
            ws: Option<String>,
            #[serde(default)]
            create_error: Option<String>,
            #[serde(flatten)]
            extra: Map<String, Value>,
        }

        let response = SessionResponse::deserialize(deserializer)?;
        let session_id = response
            .session_id
            .or(response.id)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| serde::de::Error::missing_field("session_id or id"))?;
        Ok(Self {
            session_id,
            status: response.status.unwrap_or_default(),
            project_id: response.project_id.unwrap_or_default(),
            browser_mode: response
                .browser_mode
                .or(response.browser_type)
                .unwrap_or_default(),
            region_id: response.region_id,
            context_id: response.context_id,
            context_description: response.context_description,
            context_display_name: response.context_display_name,
            created_at: response.created_at,
            inspect_url: response.inspect_url.unwrap_or_default(),
            container_id: response.container_id,
            ws: response.ws,
            create_error: response.create_error,
            extra: response.extra,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Pagination {
    #[serde(default = "one", rename = "currentPage")]
    pub current_page: u64,
    #[serde(default, rename = "pageSize")]
    pub page_size: u64,
    #[serde(default, rename = "totalCount")]
    pub total_count: u64,
    #[serde(default = "one", rename = "totalPages")]
    pub total_pages: u64,
    #[serde(default, rename = "activeCount")]
    pub active_count: u64,
    #[serde(default, rename = "closedCount")]
    pub closed_count: u64,
}

const fn one() -> u64 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionList {
    #[serde(default)]
    pub sessions: Vec<Session>,
    #[serde(default)]
    pub pagination: Pagination,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Context {
    #[serde(alias = "id")]
    pub context_id: String,
    #[serde(default, alias = "status")]
    pub locked: String,
    #[serde(default)]
    pub region_id: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub metadata: Value,
    #[serde(default)]
    pub created_at: Value,
    #[serde(default)]
    pub updated_at: Value,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ContextList {
    #[serde(default)]
    pub success: bool,
    #[serde(default)]
    pub contexts: Vec<Context>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSession {
    pub browser_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extension_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub official_proxy: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub downloads: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recording: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_image_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_size: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weak_lock: Option<bool>,
}

impl Default for CreateSession {
    fn default() -> Self {
        Self {
            browser_mode: "normal".into(),
            context: None,
            extension_ids: None,
            proxy: None,
            official_proxy: None,
            downloads: None,
            recording: None,
            custom_image_id: None,
            window_size: None,
            context_description: None,
            weak_lock: None,
        }
    }
}
