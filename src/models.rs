use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Session {
    #[serde(alias = "id")]
    pub session_id: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub project_id: String,
    #[serde(default, alias = "browser_type")]
    pub browser_mode: String,
    #[serde(default)]
    pub region_id: Option<String>,
    #[serde(default)]
    pub context_id: Option<String>,
    #[serde(default)]
    pub context_description: Option<String>,
    #[serde(default)]
    pub context_display_name: Option<String>,
    #[serde(default)]
    pub created_at: Value,
    #[serde(default)]
    pub inspect_url: String,
    #[serde(default)]
    pub container_id: Option<String>,
    #[serde(default)]
    pub ws: Option<String>,
    #[serde(default)]
    pub create_error: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
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
