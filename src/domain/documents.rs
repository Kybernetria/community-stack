use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DocumentKey {
    pub app_id: String,
    pub community_id: String,
    pub document_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MutateDocument {
    pub community_id: String,
    pub document_id: String,
    pub idempotency_key: String,
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
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
