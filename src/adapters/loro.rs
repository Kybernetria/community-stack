use anyhow::{Context, Result, bail};
use loro::{ExportMode, LoroDoc, LoroValue, ToJson, VersionVector};
use serde_json::Value;

use crate::{
    domain::{DocumentChange, MAX_UPDATE_BYTES, Mutation, PrimitiveValue},
    ports::DocumentEngine,
};

#[derive(Default)]
pub struct LoroDocumentEngine;

impl DocumentEngine for LoroDocumentEngine {
    fn read_state(&self, updates: &[Vec<u8>], writer_peer_id: u64) -> Result<Value> {
        Ok(rebuild(updates, writer_peer_id)?
            .get_deep_value()
            .to_json_value())
    }

    fn stage_mutations(
        &self,
        updates: &[Vec<u8>],
        writer_peer_id: u64,
        mutations: &[Mutation],
    ) -> Result<DocumentChange> {
        let current = rebuild(updates, writer_peer_id)?;
        let (_staged, update_bytes, state) = stage_mutations(&current, writer_peer_id, mutations)?;
        Ok(DocumentChange {
            update_bytes,
            state,
        })
    }
}

fn rebuild(updates: &[Vec<u8>], writer_peer_id: u64) -> Result<LoroDoc> {
    let doc = LoroDoc::new();
    if !updates.is_empty() {
        let status = doc
            .import_batch(updates)
            .context("stored Loro updates cannot be replayed")?;
        if status.pending.is_some() {
            bail!("stored Loro history has unresolved causal dependencies");
        }
    }
    doc.set_peer_id(writer_peer_id)
        .context("could not assign process-local Loro PeerID")?;
    Ok(doc)
}

fn stage_mutations(
    current: &LoroDoc,
    writer_peer_id: u64,
    mutations: &[Mutation],
) -> Result<(LoroDoc, Vec<u8>, Value)> {
    if mutations.is_empty() {
        bail!("at least one mutation is required");
    }
    if mutations.len() > 1_024 {
        bail!("mutation count exceeds 1024");
    }

    let before: VersionVector = current.oplog_vv();
    let staged = current.fork();
    staged
        .set_peer_id(writer_peer_id)
        .context("could not preserve the document writer session")?;

    for mutation in mutations {
        apply(&staged, mutation)?;
    }
    staged.commit();
    let update = staged
        .export(ExportMode::updates(&before))
        .context("Loro update export failed")?;
    if update.is_empty() {
        bail!("mutation produced no document change");
    }
    if update.len() > MAX_UPDATE_BYTES {
        bail!("resulting Loro update exceeds {MAX_UPDATE_BYTES} bytes");
    }
    let state = staged.get_deep_value().to_json_value();
    Ok((staged, update, state))
}

fn apply(doc: &LoroDoc, mutation: &Mutation) -> Result<()> {
    match mutation {
        Mutation::MapSet {
            container,
            key,
            value,
        } => {
            validate_name(container, "container")?;
            validate_name(key, "key")?;
            doc.get_map(container.as_str())
                .insert(key, primitive(value))
                .context("map set failed")?;
        }
        Mutation::MapDelete { container, key } => {
            validate_name(container, "container")?;
            validate_name(key, "key")?;
            doc.get_map(container.as_str())
                .delete(key)
                .context("map delete failed")?;
        }
        Mutation::TextInsert {
            container,
            index,
            text,
        } => {
            validate_name(container, "container")?;
            if text.len() > MAX_UPDATE_BYTES {
                bail!("inserted text is too large");
            }
            doc.get_text(container.as_str())
                .insert(*index, text)
                .context("text insert failed")?;
        }
        Mutation::TextDelete {
            container,
            index,
            length,
        } => {
            validate_name(container, "container")?;
            doc.get_text(container.as_str())
                .delete(*index, *length)
                .context("text delete failed")?;
        }
        Mutation::ListInsert {
            container,
            index,
            value,
        } => {
            validate_name(container, "container")?;
            doc.get_list(container.as_str())
                .insert(*index, primitive(value))
                .context("list insert failed")?;
        }
        Mutation::ListDelete {
            container,
            index,
            length,
        } => {
            validate_name(container, "container")?;
            doc.get_list(container.as_str())
                .delete(*index, *length)
                .context("list delete failed")?;
        }
    }
    Ok(())
}

fn primitive(value: &PrimitiveValue) -> LoroValue {
    match value {
        PrimitiveValue::Null => LoroValue::Null,
        PrimitiveValue::Bool(value) => (*value).into(),
        PrimitiveValue::Integer(value) => (*value).into(),
        PrimitiveValue::Float(value) => (*value).into(),
        PrimitiveValue::String(value) => value.clone().into(),
    }
}

fn validate_name(value: &str, field: &str) -> Result<()> {
    if value.is_empty() || value.len() > 256 {
        bail!("{field} must contain 1..=256 bytes");
    }
    if value.chars().any(char::is_control) {
        bail!("{field} contains a control character");
    }
    Ok(())
}
