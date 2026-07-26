use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PLANNING_PROFILE_ID: &str = "community.planning";
pub const PLANNING_PROFILE_VERSION: u32 = 1;
pub const PLANNING_PROFILE_DIGEST: &str =
    "771d32c6a851318c84139c924c573eac69d9ab8c7edca4f179afe344c48879ee";
pub const PLANNING_NAMESPACE: &str = "community.planning";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetProfile {
    pub profile_id: String,
    #[serde(default)]
    pub profile_version: Option<u32>,
    #[serde(default)]
    pub community_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantProfile {
    pub app_id: String,
    pub profile_id: String,
    pub community_id: String,
    #[serde(default)]
    pub can_read: bool,
    #[serde(default)]
    pub can_write: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileAccess {
    pub can_read: bool,
    pub can_write: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProfileManifest {
    pub profile_id: String,
    pub profile_version: u32,
    pub profile_digest: String,
    pub data_namespace: String,
    pub title: String,
    pub record_types: Vec<String>,
    pub extension_policy: String,
    pub temporal_policy: Value,
    pub contract: Value,
    pub native: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProfileDescription {
    #[serde(flatten)]
    pub manifest: ProfileManifest,
    pub access: ProfileAccess,
}
