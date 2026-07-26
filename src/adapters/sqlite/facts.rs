use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params, types::Type};

use crate::domain::{
    CheckStatus, DoctorCheck, DocumentReplayInput, FactClaim, FactCurrentRevision, FactHistoryPage,
    FactHistoryRequest, FactInspection, FactLifecyclePolicy, FactLifecycleStatus, FactRevision,
    FactSchema, GetFactSchema, InspectFact, ListFactSchemas, PLANNING_NAMESPACE,
    PLANNING_PROFILE_DIGEST, ProjectionCheck, ProvenancePolicy, RegisterFactSchema,
};

use super::now_ms;

const MAX_EXAMPLES: usize = 10;
const MAX_COUNT: i64 = 1_000_000;

pub(super) fn lifecycle(value: &str) -> rusqlite::Result<FactLifecycleStatus> {
    match value {
        "ASSERTED" => Ok(FactLifecycleStatus::Asserted),
        "SUPERSEDED" => Ok(FactLifecycleStatus::Superseded),
        "RETRACTED" => Ok(FactLifecycleStatus::Retracted),
        "DISPUTED" => Ok(FactLifecycleStatus::Disputed),
        "EXPIRED" => Ok(FactLifecycleStatus::Expired),
        _ => Err(rusqlite::Error::FromSqlConversionFailure(
            0,
            Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid fact lifecycle status",
            )),
        )),
    }
}

pub(super) fn register_schema(
    connection: &mut Connection,
    request: &RegisterFactSchema,
) -> Result<FactSchema> {
    let now = now_ms()?;
    let object_schema = serde_json::to_vec(&request.object_schema)?;
    let lifecycle = serde_json::to_vec(&request.lifecycle_policy)?;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute(
        "INSERT INTO fact_schemas(app_id,schema_id,schema_version,predicate,object_schema_json,multiple_active_claims,provenance_policy,lifecycle_policy_json,created_at_ms,enabled) \
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![
            request.app_id,
            request.schema_id,
            i64::from(request.schema_version),
            request.predicate,
            object_schema,
            i64::from(request.multiple_active_claims),
            request.provenance_policy.as_str(),
            lifecycle,
            now,
            i64::from(request.enabled)
        ],
    )?;
    tx.execute(
        "INSERT INTO audit_events(event_kind,app_id,object_id,redacted_detail,created_at_ms) VALUES('FACT_SCHEMA_REGISTERED',?1,?2,'versioned bounded schema registered',?3)",
        params![request.app_id, request.schema_id, now],
    )?;
    tx.commit()?;
    Ok(FactSchema {
        app_id: request.app_id.clone(),
        schema_id: request.schema_id.clone(),
        schema_version: request.schema_version,
        predicate: request.predicate.clone(),
        object_schema: request.object_schema.clone(),
        multiple_active_claims: request.multiple_active_claims,
        provenance_policy: request.provenance_policy,
        lifecycle_policy: request.lifecycle_policy.clone(),
        created_at_ms: now,
        enabled: request.enabled,
    })
}

pub(super) fn get_schema(
    connection: &Connection,
    request: &GetFactSchema,
) -> Result<Option<FactSchema>> {
    let version = request.schema_version.map(i64::from);
    connection
        .query_row(
            "SELECT app_id,schema_id,schema_version,predicate,object_schema_json,multiple_active_claims,provenance_policy,lifecycle_policy_json,created_at_ms,enabled \
             FROM fact_schemas WHERE app_id=?1 AND schema_id=?2 AND (?3 IS NULL OR schema_version=?3) \
             ORDER BY schema_version DESC LIMIT 1",
            params![request.app_id, request.schema_id, version],
            schema_from_row,
        )
        .optional()
        .map_err(Into::into)
}

pub(super) fn list_schemas(
    connection: &Connection,
    request: &ListFactSchemas,
) -> Result<Vec<FactSchema>> {
    let mut statement = connection.prepare(
        "SELECT app_id,schema_id,schema_version,predicate,object_schema_json,multiple_active_claims,provenance_policy,lifecycle_policy_json,created_at_ms,enabled \
         FROM fact_schemas WHERE app_id=?1 ORDER BY schema_id,schema_version DESC LIMIT ?2",
    )?;
    let rows = statement.query_map(
        params![request.app_id, i64::from(request.limit.min(500))],
        schema_from_row,
    )?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

fn schema_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<FactSchema> {
    let object_bytes: Vec<u8> = row.get(4)?;
    let lifecycle_bytes: Vec<u8> = row.get(7)?;
    let object_schema = serde_json::from_slice(&object_bytes).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(4, Type::Blob, Box::new(error))
    })?;
    let lifecycle_policy = serde_json::from_slice::<FactLifecyclePolicy>(&lifecycle_bytes)
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(7, Type::Blob, Box::new(error))
        })?;
    let provenance: String = row.get(6)?;
    let provenance_policy = match provenance.as_str() {
        "NONE" => ProvenancePolicy::None,
        "SOURCE" => ProvenancePolicy::Source,
        "SOURCE_DOCUMENT" => ProvenancePolicy::SourceDocument,
        "SOURCE_AND_DOCUMENT" => ProvenancePolicy::SourceAndDocument,
        _ => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                6,
                Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid provenance policy",
                )),
            ));
        }
    };
    Ok(FactSchema {
        app_id: row.get(0)?,
        schema_id: row.get(1)?,
        schema_version: row.get(2)?,
        predicate: row.get(3)?,
        object_schema,
        multiple_active_claims: row.get(5)?,
        provenance_policy,
        lifecycle_policy,
        created_at_ms: row.get(8)?,
        enabled: row.get(9)?,
    })
}

pub(super) fn current_revision(
    connection: &Connection,
    app_id: &str,
    community_id: &str,
    claim_id: &str,
) -> Result<Option<FactCurrentRevision>> {
    connection
        .query_row(
            "SELECT lower(hex(source_operation_hash)),subject,predicate,schema_id,schema_version,lifecycle_status FROM fact_claims \
             WHERE app_id=?1 AND community_id=?2 AND claim_id=?3",
            params![app_id, community_id, claim_id],
            |row| {
                Ok(FactCurrentRevision {
                    operation_hash: row.get(0)?,
                    subject: row.get(1)?,
                    predicate: row.get(2)?,
                    schema_id: row.get(3)?,
                    schema_version: row.get(4)?,
                    lifecycle_status: lifecycle(row.get::<_, String>(5)?.as_str())?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
}

pub(super) fn active_conflict(
    connection: &Connection,
    app_id: &str,
    community_id: &str,
    claim_id: &str,
    subject: &str,
    predicate: &str,
) -> Result<bool> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM fact_claims WHERE app_id=?1 AND community_id=?2 AND claim_id!=?3 AND subject=?4 AND predicate=?5 AND retracted=0)",
            params![app_id, community_id, claim_id, subject, predicate],
            |row| row.get(0),
        )
        .map_err(Into::into)
}

pub(super) fn history(
    connection: &Connection,
    app_id: &str,
    request: &FactHistoryRequest,
) -> Result<FactHistoryPage> {
    let (cursor_ms, cursor_hash) = request
        .cursor
        .as_deref()
        .map(parse_cursor)
        .transpose()?
        .map_or((None, None), |(time, hash)| (Some(time), Some(hash)));
    let fetch = i64::from(request.limit.min(100)) + 1;
    let mut statement = connection.prepare(
        "SELECT lower(hex(operation_hash)),claim_id,community_id,subject,predicate,object_json,source,confidence,lower(hex(author_key)),schema_id,schema_version,asserted_at_ms,lower(hex(supersedes_operation_hash)),lifecycle_status,source_document_id \
         FROM fact_claim_revisions WHERE app_id=?1 AND community_id=?2 AND claim_id=?3 \
         AND (?4 IS NULL OR asserted_at_ms<?4 OR (asserted_at_ms=?4 AND lower(hex(operation_hash))<?5)) \
         ORDER BY asserted_at_ms DESC,operation_hash DESC LIMIT ?6",
    )?;
    let rows = statement.query_map(
        params![
            app_id,
            request.community_id,
            request.claim_id,
            cursor_ms,
            cursor_hash,
            fetch
        ],
        revision_from_row,
    )?;
    let mut revisions = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    let has_more = revisions.len() > usize::from(request.limit);
    if has_more {
        revisions.pop();
    }
    let next_cursor = if has_more {
        revisions
            .last()
            .map(|revision| format!("{}:{}", revision.asserted_at_ms, revision.operation_hash))
    } else {
        None
    };
    Ok(FactHistoryPage {
        revisions,
        next_cursor,
        limit: request.limit,
    })
}

fn parse_cursor(cursor: &str) -> Result<(i64, String)> {
    let (time, hash) = cursor
        .split_once(':')
        .context("history cursor is malformed")?;
    let time = time.parse::<i64>().context("history cursor is malformed")?;
    let bytes = hex::decode(hash).context("history cursor is malformed")?;
    if bytes.len() != 32 || hash.len() != 64 {
        bail!("history cursor is malformed");
    }
    Ok((time, hash.to_ascii_lowercase()))
}

fn revision_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<FactRevision> {
    let object_bytes: Vec<u8> = row.get(5)?;
    let object = serde_json::from_slice(&object_bytes).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(5, Type::Blob, Box::new(error))
    })?;
    Ok(FactRevision {
        operation_hash: row.get(0)?,
        claim_id: row.get(1)?,
        community_id: row.get(2)?,
        subject: row.get(3)?,
        predicate: row.get(4)?,
        object,
        source: row.get(6)?,
        confidence: row.get(7)?,
        author_key: row.get(8)?,
        schema_id: row.get(9)?,
        schema_version: row.get(10)?,
        asserted_at_ms: row.get(11)?,
        supersedes_operation_hash: row.get(12)?,
        lifecycle_status: lifecycle(row.get::<_, String>(13)?.as_str())?,
        source_document_id: row.get(14)?,
    })
}

pub(super) fn inspect(
    connection: &Connection,
    app_id: &str,
    request: &InspectFact,
) -> Result<Option<FactInspection>> {
    let row = connection
        .query_row(
            "SELECT c.claim_id,c.community_id,c.subject,c.predicate,c.object_json,c.source,c.confidence,lower(hex(c.source_operation_hash)),c.updated_at_ms,c.schema_id,c.schema_version,c.lifecycle_status,c.source_document_id, \
                    lower(hex(r.author_key)),r.subject,r.predicate,r.object_json,r.source,r.confidence,r.schema_id,r.schema_version,r.lifecycle_status,r.source_document_id,lower(hex(r.supersedes_operation_hash)), \
                    o.verification_status,o.apply_status,o.document_id \
             FROM fact_claims c JOIN fact_claim_revisions r ON r.operation_hash=c.source_operation_hash \
             JOIN operations o ON o.operation_hash=r.operation_hash \
             WHERE c.app_id=?1 AND c.community_id=?2 AND c.claim_id=?3",
            params![app_id, request.community_id, request.claim_id],
            |row| {
                let current_object: Vec<u8> = row.get(4)?;
                let revision_object: Vec<u8> = row.get(16)?;
                let current = FactClaim {
                    claim_id: row.get(0)?,
                    community_id: row.get(1)?,
                    subject: row.get(2)?,
                    predicate: row.get(3)?,
                    object: serde_json::from_slice(&current_object).map_err(|error| rusqlite::Error::FromSqlConversionFailure(4,Type::Blob,Box::new(error)))?,
                    source: row.get(5)?,
                    confidence: row.get(6)?,
                    source_operation_hash: row.get(7)?,
                    updated_at_ms: row.get(8)?,
                    schema_id: row.get(9)?,
                    schema_version: row.get(10)?,
                    lifecycle_status: lifecycle(row.get::<_, String>(11)?.as_str())?,
                    source_document_id: row.get(12)?,
                };
                let mut issues = Vec::new();
                if current.subject != row.get::<_, String>(14)? { issues.push("subject differs from revision".into()); }
                if current.predicate != row.get::<_, String>(15)? { issues.push("predicate differs from revision".into()); }
                if current_object != revision_object { issues.push("object hash differs from revision".into()); }
                if current.source != row.get::<_, Option<String>>(17)? { issues.push("source differs from revision".into()); }
                if (current.confidence - row.get::<_, f64>(18)?).abs() > f64::EPSILON { issues.push("confidence differs from revision".into()); }
                if current.schema_id != row.get::<_, String>(19)? || current.schema_version != row.get::<_, u32>(20)? { issues.push("schema reference differs from revision".into()); }
                if current.lifecycle_status != lifecycle(row.get::<_, String>(21)?.as_str())? { issues.push("lifecycle differs from revision".into()); }
                if current.source_document_id != row.get::<_, Option<String>>(22)? { issues.push("source document differs from revision".into()); }
                Ok((current,row.get::<_,String>(13)?,row.get::<_,Option<String>>(23)?,row.get::<_,String>(24)?,row.get::<_,String>(25)?,row.get::<_,String>(26)?,issues))
            },
        )
        .optional()?;
    let Some((current, author_key, supersedes, verification, apply, document, issues)) = row else {
        return Ok(None);
    };
    let revision_count: u32 = connection.query_row(
        "SELECT count(*) FROM fact_claim_revisions WHERE app_id=?1 AND community_id=?2 AND claim_id=?3",
        params![app_id, request.community_id, request.claim_id],
        |row| row.get(0),
    )?;
    let superseded_by_operation_hash = connection
        .query_row(
            "SELECT lower(hex(operation_hash)) FROM fact_claim_revisions WHERE supersedes_operation_hash=?1 ORDER BY asserted_at_ms DESC,operation_hash DESC LIMIT 1",
            [hex::decode(&current.source_operation_hash)?],
            |row| row.get(0),
        )
        .optional()?;
    Ok(Some(FactInspection {
        current_revision_operation_hash: current.source_operation_hash.clone(),
        author_key,
        semantic_source: current.source.clone(),
        source_document_id: current.source_document_id.clone(),
        schema_id: current.schema_id.clone(),
        schema_version: current.schema_version,
        lifecycle_status: current.lifecycle_status,
        loro_document_id: document,
        revision_count,
        verification_status: verification,
        apply_status: apply,
        supersedes_operation_hash: supersedes,
        superseded_by_operation_hash,
        projection_consistent: issues.is_empty(),
        consistency_issues: issues,
        current_claim: current,
    }))
}

pub(super) fn replay_inputs(
    connection: &Connection,
    limit: u16,
) -> Result<Vec<DocumentReplayInput>> {
    let mut documents = connection.prepare(
        "SELECT app_id,community_id,document_id FROM document_updates WHERE applied=1 \
         GROUP BY app_id,community_id,document_id ORDER BY app_id,community_id,document_id LIMIT ?1",
    )?;
    let keys = documents
        .query_map([i64::from(limit.min(101))], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut result = Vec::with_capacity(keys.len());
    for (app, community, document) in keys {
        let mut statement = connection.prepare(
            "SELECT update_bytes FROM document_updates WHERE app_id=?1 AND community_id=?2 AND document_id=?3 AND applied=1 ORDER BY created_at_ms,operation_hash",
        )?;
        let updates = statement
            .query_map(params![app, community, document], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        result.push(DocumentReplayInput {
            identifier: format!("{app}/{community}/{document}"),
            updates,
        });
    }
    Ok(result)
}

pub(super) fn projection_check(connection: &Connection) -> Result<ProjectionCheck> {
    let current = projection_rows(connection, false)?;
    let expected = projection_rows(connection, true)?;
    let current_hash = rows_hash(&current);
    let expected_hash = rows_hash(&expected);
    let mut mismatches = Vec::new();
    for key in current.keys().chain(expected.keys()) {
        if current.get(key) != expected.get(key) && !mismatches.contains(key) {
            mismatches.push(key.clone());
            if mismatches.len() == MAX_EXAMPLES {
                break;
            }
        }
    }
    Ok(ProjectionCheck {
        projection: "fact_claims".into(),
        consistent: current == expected,
        current_count: u32::try_from(current.len()).unwrap_or(u32::MAX),
        expected_count: u32::try_from(expected.len()).unwrap_or(u32::MAX),
        current_hash,
        expected_hash,
        mismatches,
        rebuild_enabled: false,
    })
}

fn projection_rows(connection: &Connection, expected: bool) -> Result<BTreeMap<String, Vec<u8>>> {
    let sql = if expected {
        "SELECT r.app_id,r.community_id,r.claim_id,r.subject,r.predicate,r.object_json,r.source,r.confidence,r.operation_hash,CASE WHEN r.lifecycle_status='ASSERTED' THEN 0 ELSE 1 END,r.asserted_at_ms,r.schema_id,r.schema_version,r.lifecycle_status,r.source_document_id \
         FROM fact_claim_revisions r WHERE NOT EXISTS(SELECT 1 FROM fact_claim_revisions newer WHERE newer.app_id=r.app_id AND newer.community_id=r.community_id AND newer.claim_id=r.claim_id AND (newer.asserted_at_ms>r.asserted_at_ms OR (newer.asserted_at_ms=r.asserted_at_ms AND newer.operation_hash>r.operation_hash))) ORDER BY r.app_id,r.community_id,r.claim_id"
    } else {
        "SELECT app_id,community_id,claim_id,subject,predicate,object_json,source,confidence,source_operation_hash,retracted,updated_at_ms,schema_id,schema_version,lifecycle_status,source_document_id FROM fact_claims ORDER BY app_id,community_id,claim_id"
    };
    let mut statement = connection.prepare(sql)?;
    let rows = statement.query_map([], |row| {
        let app: String = row.get(0)?;
        let community: String = row.get(1)?;
        let claim: String = row.get(2)?;
        let mut encoded = Vec::new();
        for bytes in [
            app.as_bytes().to_vec(),
            community.as_bytes().to_vec(),
            claim.as_bytes().to_vec(),
            row.get::<_, String>(3)?.into_bytes(),
            row.get::<_, String>(4)?.into_bytes(),
            row.get::<_, Vec<u8>>(5)?,
            row.get::<_, Option<String>>(6)?
                .unwrap_or_default()
                .into_bytes(),
            row.get::<_, f64>(7)?.to_bits().to_be_bytes().to_vec(),
            row.get::<_, Vec<u8>>(8)?,
            row.get::<_, i64>(9)?.to_be_bytes().to_vec(),
            row.get::<_, i64>(10)?.to_be_bytes().to_vec(),
            row.get::<_, String>(11)?.into_bytes(),
            row.get::<_, i64>(12)?.to_be_bytes().to_vec(),
            row.get::<_, String>(13)?.into_bytes(),
            row.get::<_, Option<String>>(14)?
                .unwrap_or_default()
                .into_bytes(),
        ] {
            encoded.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
            encoded.extend_from_slice(&bytes);
        }
        Ok((format!("{app}/{community}/{claim}"), encoded))
    })?;
    rows.collect::<rusqlite::Result<BTreeMap<_, _>>>()
        .map_err(Into::into)
}

fn rows_hash(rows: &BTreeMap<String, Vec<u8>>) -> String {
    let mut hasher = blake3::Hasher::new();
    for (key, value) in rows {
        hasher.update(&(key.len() as u64).to_be_bytes());
        hasher.update(key.as_bytes());
        hasher.update(&(value.len() as u64).to_be_bytes());
        hasher.update(value);
    }
    hasher.finalize().to_hex().to_string()
}

pub(super) fn doctor(connection: &Connection) -> Result<Vec<DoctorCheck>> {
    let mut checks = Vec::new();
    let quick: String = connection.query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
    let integrity: String =
        connection.query_row("PRAGMA integrity_check(1)", [], |row| row.get(0))?;
    checks.push(simple_check(
        "sqlite_integrity",
        i64::from(quick != "ok" || integrity != "ok"),
        if quick == "ok" && integrity == "ok" {
            vec![]
        } else {
            vec!["sqlite-check-failed".into()]
        },
        false,
    ));

    let mut foreign_examples = Vec::new();
    let mut foreign_count = 0_i64;
    let mut statement = connection.prepare("PRAGMA foreign_key_check")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    for row in rows {
        let (table, rowid) = row?;
        foreign_count += 1;
        if foreign_examples.len() < MAX_EXAMPLES {
            foreign_examples.push(format!("{table}:{rowid}"));
        }
    }
    checks.push(simple_check(
        "foreign_keys",
        foreign_count,
        foreign_examples,
        false,
    ));
    checks.push(query_check(connection, "operation_body_references",
        "SELECT (SELECT count(*) FROM operations WHERE length(canonical_header)=0 OR length(body_ciphertext)=0)+(SELECT count(*) FROM document_updates d LEFT JOIN operations o ON o.operation_hash=d.operation_hash WHERE o.operation_hash IS NULL)",
        "SELECT lower(hex(operation_hash)) FROM operations WHERE length(canonical_header)=0 OR length(body_ciphertext)=0 UNION ALL SELECT lower(hex(d.operation_hash)) FROM document_updates d LEFT JOIN operations o ON o.operation_hash=d.operation_hash WHERE o.operation_hash IS NULL LIMIT 10", false)?);
    checks.push(query_check(connection, "author_log_continuity",
        "SELECT count(*) FROM operations o WHERE (o.sequence=0 AND o.backlink IS NOT NULL) OR (o.sequence>0 AND (o.backlink IS NULL OR NOT EXISTS(SELECT 1 FROM operations p WHERE p.operation_hash=o.backlink AND p.author_key=o.author_key AND p.log_id=o.log_id AND p.generation=o.generation AND p.sequence=o.sequence-1)))",
        "SELECT lower(hex(o.operation_hash)) FROM operations o WHERE (o.sequence=0 AND o.backlink IS NOT NULL) OR (o.sequence>0 AND (o.backlink IS NULL OR NOT EXISTS(SELECT 1 FROM operations p WHERE p.operation_hash=o.backlink AND p.author_key=o.author_key AND p.log_id=o.log_id AND p.generation=o.generation AND p.sequence=o.sequence-1))) LIMIT 10", false)?);
    checks.push(query_check(connection, "applied_with_pending_dependencies",
        "SELECT count(*) FROM operations o JOIN document_updates d USING(operation_hash) WHERE o.apply_status='APPLIED' AND (d.applied=0 OR d.pending_deps IS NOT NULL)",
        "SELECT lower(hex(o.operation_hash)) FROM operations o JOIN document_updates d USING(operation_hash) WHERE o.apply_status='APPLIED' AND (d.applied=0 OR d.pending_deps IS NOT NULL) LIMIT 10", false)?);
    checks.push(query_check(connection, "current_revision_references",
        "SELECT count(*) FROM fact_claims c LEFT JOIN fact_claim_revisions r ON r.operation_hash=c.source_operation_hash WHERE r.operation_hash IS NULL",
        "SELECT c.app_id||'/'||c.community_id||'/'||c.claim_id FROM fact_claims c LEFT JOIN fact_claim_revisions r ON r.operation_hash=c.source_operation_hash WHERE r.operation_hash IS NULL LIMIT 10", true)?);
    checks.push(query_check(connection, "revision_chain_links",
        "SELECT count(*) FROM fact_claim_revisions r LEFT JOIN fact_claim_revisions p ON p.operation_hash=r.supersedes_operation_hash WHERE r.supersedes_operation_hash IS NOT NULL AND (p.operation_hash IS NULL OR p.app_id!=r.app_id OR p.community_id!=r.community_id OR p.claim_id!=r.claim_id)",
        "SELECT lower(hex(r.operation_hash)) FROM fact_claim_revisions r LEFT JOIN fact_claim_revisions p ON p.operation_hash=r.supersedes_operation_hash WHERE r.supersedes_operation_hash IS NOT NULL AND (p.operation_hash IS NULL OR p.app_id!=r.app_id OR p.community_id!=r.community_id OR p.claim_id!=r.claim_id) LIMIT 10", true)?);
    checks.push(query_check(connection, "revision_chain_cycles",
        "WITH RECURSIVE c(start,current,depth) AS (SELECT operation_hash,supersedes_operation_hash,0 FROM fact_claim_revisions UNION ALL SELECT c.start,r.supersedes_operation_hash,c.depth+1 FROM c JOIN fact_claim_revisions r ON r.operation_hash=c.current WHERE c.current IS NOT NULL AND c.depth<1000) SELECT count(DISTINCT start) FROM c WHERE current=start",
        "WITH RECURSIVE c(start,current,depth) AS (SELECT operation_hash,supersedes_operation_hash,0 FROM fact_claim_revisions UNION ALL SELECT c.start,r.supersedes_operation_hash,c.depth+1 FROM c JOIN fact_claim_revisions r ON r.operation_hash=c.current WHERE c.current IS NOT NULL AND c.depth<1000) SELECT lower(hex(start)) FROM c WHERE current=start LIMIT 10", true)?);
    let projection = projection_check(connection)?;
    checks.push(simple_check(
        "current_projection_consistency",
        i64::from(!projection.consistent),
        projection.mismatches,
        true,
    ));
    checks.push(query_check(
        connection,
        "planning_profile_binding",
        &format!(
            "SELECT count(*) FROM (SELECT profile_digest,source_operation_hash FROM planning_projects UNION ALL SELECT profile_digest,source_operation_hash FROM planning_tasks UNION ALL SELECT profile_digest,source_operation_hash FROM planning_events UNION ALL SELECT profile_digest,source_operation_hash FROM planning_dependencies) p JOIN operations o ON o.operation_hash=p.source_operation_hash WHERE p.profile_digest!='{PLANNING_PROFILE_DIGEST}' OR o.app_id!='{PLANNING_NAMESPACE}'"
        ),
        &format!(
            "SELECT lower(hex(p.source_operation_hash)) FROM (SELECT profile_digest,source_operation_hash FROM planning_projects UNION ALL SELECT profile_digest,source_operation_hash FROM planning_tasks UNION ALL SELECT profile_digest,source_operation_hash FROM planning_events UNION ALL SELECT profile_digest,source_operation_hash FROM planning_dependencies) p JOIN operations o ON o.operation_hash=p.source_operation_hash WHERE p.profile_digest!='{PLANNING_PROFILE_DIGEST}' OR o.app_id!='{PLANNING_NAMESPACE}' LIMIT 10"
        ),
        true,
    )?);
    checks.push(query_check(connection, "orphaned_outbox_ack_rows",
        "SELECT (SELECT count(*) FROM durable_outbox q LEFT JOIN operations o ON o.operation_hash=q.operation_hash WHERE o.operation_hash IS NULL)+(SELECT count(*) FROM transport_deliveries d LEFT JOIN operations o ON o.operation_hash=d.operation_hash WHERE o.operation_hash IS NULL)+(SELECT count(*) FROM application_acks a LEFT JOIN operations o ON o.operation_hash=a.operation_hash WHERE o.operation_hash IS NULL)",
        "SELECT 'orphan-reference' WHERE (SELECT count(*) FROM durable_outbox q LEFT JOIN operations o ON o.operation_hash=q.operation_hash WHERE o.operation_hash IS NULL)+(SELECT count(*) FROM transport_deliveries d LEFT JOIN operations o ON o.operation_hash=d.operation_hash WHERE o.operation_hash IS NULL)>0 LIMIT 1", true)?);
    checks.push(query_check(connection, "migration_state",
        "SELECT CASE WHEN (SELECT count(*) FROM schema_migrations WHERE version IN (1,2,3,4,5))=5 AND (SELECT max(version) FROM schema_migrations)=5 AND (SELECT count(*) FROM schema_migrations WHERE checksum IS NULL OR length(checksum)!=64)=0 THEN 0 ELSE 1 END",
        "SELECT 'unexpected-migration-state' WHERE (SELECT count(*) FROM schema_migrations WHERE version IN (1,2,3,4,5))!=5 OR (SELECT max(version) FROM schema_migrations)!=5 OR (SELECT count(*) FROM schema_migrations WHERE checksum IS NULL OR length(checksum)!=64)>0", false)?);
    Ok(checks)
}

fn query_check(
    connection: &Connection,
    name: &str,
    count_sql: &str,
    examples_sql: &str,
    repairable: bool,
) -> Result<DoctorCheck> {
    let count: i64 = connection.query_row(count_sql, [], |row| row.get(0))?;
    let mut statement = connection.prepare(examples_sql)?;
    let examples = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .take(MAX_EXAMPLES)
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(simple_check(name, count, examples, repairable))
}

fn simple_check(name: &str, count: i64, examples: Vec<String>, repairable: bool) -> DoctorCheck {
    DoctorCheck {
        check_name: name.into(),
        status: if count == 0 {
            CheckStatus::Ok
        } else {
            CheckStatus::Error
        },
        count: u32::try_from(count.clamp(0, MAX_COUNT)).unwrap_or(u32::MAX),
        examples,
        repairable,
    }
}
