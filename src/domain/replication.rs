use serde::{Deserialize, Serialize};

use super::DocumentKey;

#[derive(Clone, Debug)]
pub struct LogHead {
    pub sequence: u32,
    pub operation_hash: [u8; 32],
}

#[derive(Clone, Debug)]
pub struct RecordMetadata {
    pub app_id: String,
    pub community_id: String,
    pub document_id: String,
    pub record_kind: u8,
    pub codec_version: u8,
    pub key_epoch: u32,
}

#[derive(Clone, Debug)]
pub struct CanonicalRecord {
    pub operation_hash: [u8; 32],
    pub canonical_header: Vec<u8>,
    pub body_ciphertext: Vec<u8>,
    pub author_key: [u8; 32],
    pub log_id: [u8; 32],
    pub sequence: u32,
    pub backlink: Option<[u8; 32]>,
    pub metadata: RecordMetadata,
}

pub struct ForgeDocumentUpdate<'a> {
    pub key: &'a DocumentKey,
    pub sequence: u32,
    pub backlink: Option<[u8; 32]>,
    pub schema_version: u32,
    pub key_epoch: u32,
    pub auth_frontier: Vec<u8>,
    pub loro_update: &'a [u8],
    pub semantic_transaction: &'a str,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClaimOutbox {
    pub transport: String,
    #[serde(default = "default_claim_limit")]
    pub limit: u16,
    #[serde(default = "default_claim_bytes")]
    pub max_bytes: u32,
}

const fn default_claim_limit() -> u16 {
    32
}
const fn default_claim_bytes() -> u32 {
    262_144
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AckOutbox {
    pub transport: String,
    pub operation_hash: String,
    pub lease_attempt: u32,
    pub status: DeliveryAck,
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DeliveryAck {
    Stored,
    Applied,
    PendingDeps,
    Rejected,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutboxItem {
    pub operation_hash: String,
    pub lease_attempt: u32,
    pub header_base64: String,
    pub body_base64: String,
    pub priority: i64,
}
