use super::{CommunityCore, validate_object_id};
use crate::domain::{DocumentChanges, ListDocuments, PLANNING_NAMESPACE, PLANNING_PROFILE_ID};
use anyhow::{Result, bail};
use serde_json::Value;
impl CommunityCore {
    pub(super) async fn list_documents(
        &self,
        app_id: &str,
        request: ListDocuments,
    ) -> Result<Value> {
        validate_object_id(&request.community_id, "community_id")?;
        validate_limit(request.limit)?;
        if !request.prefix.is_empty() {
            validate_object_id(&request.prefix, "prefix")?;
        }
        if let Some(after) = &request.after {
            validate_object_id(after, "after")?;
        }
        Ok(serde_json::to_value(
            self.repository.list_documents(app_id, &request).await?,
        )?)
    }
    pub(super) async fn document_changes(
        &self,
        app_id: &str,
        request: DocumentChanges,
    ) -> Result<Value> {
        validate_object_id(&request.community_id, "community_id")?;
        validate_limit(request.limit)?;
        i64::try_from(request.after)?;
        let namespace = match request.profile_id.as_deref() {
            None => app_id,
            Some(PLANNING_PROFILE_ID) => {
                self.require_profile_read(app_id, &request.community_id)
                    .await?;
                PLANNING_NAMESPACE
            }
            Some(_) => bail!("unknown native profile"),
        };
        Ok(serde_json::to_value(
            self.repository
                .document_changes(namespace, &request)
                .await?,
        )?)
    }
}
fn validate_limit(limit: u16) -> Result<()> {
    if limit == 0 || limit > 500 {
        bail!("page limit must be between 1 and 500");
    }
    Ok(())
}
