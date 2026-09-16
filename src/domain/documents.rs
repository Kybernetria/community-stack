use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DocumentKey {
    pub app_id: String,
    pub community_id: String,
    pub document_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MutateDocument {
    pub community_id: String,
    pub document_id: String,
    pub idempotency_key: String,
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<u64>,
    pub mutations: Vec<Mutation>,
}

const fn default_schema_version() -> u32 {
    1
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Mutation {
    MapSet {
        container: String,
        key: String,
        value: PrimitiveValue,
    },
    MapDelete {
        container: String,
        key: String,
    },
    TextInsert {
        container: String,
        index: usize,
        text: String,
    },
    TextDelete {
        container: String,
        index: usize,
        length: usize,
    },
    ListInsert {
        container: String,
        index: usize,
        value: PrimitiveValue,
    },
    ListDelete {
        container: String,
        index: usize,
        length: usize,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PrimitiveValue {
    Null,
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GetDocument {
    pub community_id: String,
    pub document_id: String,
}

#[derive(Clone, Debug)]
pub struct DocumentChange {
    pub update_bytes: Vec<u8>,
    pub state: serde_json::Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListDocuments {
    pub community_id: String,
    #[serde(default)]
    pub prefix: String,
    pub after: Option<String>,
    #[serde(default = "default_page_limit")]
    pub limit: u16,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DocumentSummary {
    pub document_id: String,
    pub revision: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DocumentPage {
    pub documents: Vec<DocumentSummary>,
    pub next_cursor: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentChanges {
    pub community_id: String,
    pub profile_id: Option<String>,
    #[serde(default)]
    pub after: u64,
    #[serde(default = "default_page_limit")]
    pub limit: u16,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DocumentChangeEntry {
    pub cursor: u64,
    pub document_id: String,
    pub operation_hash: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DocumentChangesPage {
    pub changes: Vec<DocumentChangeEntry>,
    pub next_cursor: u64,
    pub has_more: bool,
}
const fn default_page_limit() -> u16 {
    100
}
