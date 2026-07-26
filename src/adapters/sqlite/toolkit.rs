use anyhow::{Context, Result, anyhow};
use rusqlite::{Connection, Transaction, params};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::domain::{
    ConceptDefinition, ToolkitAssertion, ToolkitCatalog, ToolkitConcept, ToolkitProjection,
    ToolkitReview, ToolkitTool,
};

pub(super) fn apply_projection(
    tx: &Transaction<'_>,
    app_id: &str,
    community_id: &str,
    operation_hash: &[u8; 32],
    projection: ToolkitProjection,
) -> Result<()> {
    match projection {
        ToolkitProjection::Concept(concept) => {
            tx.execute(
                "UPDATE toolkit_concepts SET active_revision=0 WHERE app_id=?1 AND community_id=?2 AND concept_key=?3 AND active_revision=1",
                params![app_id, community_id, concept.definition.key],
            )?;
            tx.execute(
                "INSERT INTO toolkit_concepts(app_id,community_id,concept_key,revision_hash,name,kind,definition,aliases_json,value_type,cardinality,applicable_json,allowed_values_json,scale_json,parent_key,status,active_revision,source_operation_hash) \
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,1,?16)",
                params![
                    app_id,
                    community_id,
                    concept.definition.key,
                    concept.revision,
                    concept.definition.name,
                    enum_text(&concept.definition.kind)?,
                    concept.definition.definition,
                    serde_json::to_vec(&concept.definition.aliases)?,
                    enum_text(&concept.definition.value_type)?,
                    enum_text(&concept.definition.cardinality)?,
                    serde_json::to_vec(&concept.definition.applicable_categories)?,
                    concept.definition.allowed_values.as_ref().map(serde_json::to_vec).transpose()?,
                    concept.definition.scale.as_ref().map(serde_json::to_vec).transpose()?,
                    concept.definition.parent,
                    enum_text(&concept.definition.status)?,
                    operation_hash.as_slice(),
                ],
            )?;
        }
        ToolkitProjection::Tool(tool) => {
            tx.execute(
                "INSERT INTO toolkit_tools(app_id,community_id,tool_key,name,description,homepage,status,source_operation_hash) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![app_id, community_id, tool.key, tool.name, tool.description, tool.homepage, tool.status, operation_hash.as_slice()],
            )?;
        }
        ToolkitProjection::Assertion(assertion) => {
            let (kind, number, text, boolean) = typed_value(assertion.value.as_ref())?;
            tx.execute(
                "INSERT INTO toolkit_assertions(app_id,community_id,assertion_id,tool_key,concept_key,concept_revision,value_json,value_kind,value_number,value_text,value_boolean,origin,initial_state,effective_state,source,evidence,as_of,rubric,rubric_version,rationale,evaluator_type,evaluation_date,source_operation_hash,review_operation_hash) \
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,NULL)",
                params![
                    app_id,
                    community_id,
                    assertion.assertion_id,
                    assertion.tool,
                    assertion.concept,
                    assertion.concept_revision,
                    assertion.value.as_ref().map(serde_json::to_vec).transpose()?,
                    kind,
                    number,
                    text,
                    boolean,
                    enum_text(&assertion.origin)?,
                    enum_text(&assertion.verification_state)?,
                    assertion.source,
                    assertion.evidence,
                    assertion.as_of,
                    assertion.rubric,
                    assertion.rubric_version,
                    assertion.rationale,
                    assertion.evaluator_type,
                    assertion.evaluation_date,
                    operation_hash.as_slice(),
                ],
            )?;
        }
        ToolkitProjection::Review(review) => {
            tx.execute(
                "INSERT INTO toolkit_reviews(app_id,community_id,review_id,assertion_id,state,reviewer,rationale,review_date,source_operation_hash) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![app_id, community_id, review.review_id, review.assertion_id, enum_text(&review.state)?, review.reviewer, review.rationale, review.review_date, operation_hash.as_slice()],
            )?;
            // Reviews are immutable. If replicated authorized reviews disagree,
            // aggregate deterministically to rejected (fail closed), independent
            // of insertion order. A future authorization layer may refine this.
            tx.execute(
                "UPDATE toolkit_assertions SET \
                   effective_state=(SELECT CASE WHEN count(*)=0 THEN toolkit_assertions.initial_state WHEN count(DISTINCT state)=1 THEN min(state) ELSE 'rejected' END FROM toolkit_reviews r WHERE r.app_id=?1 AND r.community_id=?2 AND r.assertion_id=?3), \
                   review_operation_hash=(SELECT source_operation_hash FROM toolkit_reviews r WHERE r.app_id=?1 AND r.community_id=?2 AND r.assertion_id=?3 ORDER BY hex(source_operation_hash) LIMIT 1) \
                 WHERE app_id=?1 AND community_id=?2 AND assertion_id=?3",
                params![app_id, community_id, review.assertion_id],
            )?;
        }
    }
    Ok(())
}

pub(super) fn concepts(
    connection: &Connection,
    app_id: &str,
    community_id: &str,
    active_only: bool,
    limit: u16,
) -> Result<Vec<ToolkitConcept>> {
    let mut statement = connection.prepare(
        "SELECT concept_key,revision_hash,name,kind,definition,aliases_json,value_type,cardinality,applicable_json,allowed_values_json,scale_json,parent_key,status,active_revision,hex(source_operation_hash) \
         FROM toolkit_concepts WHERE app_id=?1 AND community_id=?2 AND (?3=0 OR active_revision=1) \
         ORDER BY concept_key,revision_hash LIMIT ?4",
    )?;
    let rows = statement.query_map(
        params![
            app_id,
            community_id,
            i64::from(active_only),
            i64::from(limit.min(500))
        ],
        map_concept,
    )?;
    collect(rows)
}

pub(super) fn catalog(
    connection: &Connection,
    app_id: &str,
    community_id: &str,
) -> Result<ToolkitCatalog> {
    Ok(ToolkitCatalog {
        concepts: query_concepts(connection, app_id, community_id, None, 0)?,
        tools: query_tools(connection, app_id, community_id, None, 0)?,
        assertions: query_assertions(connection, app_id, community_id, None, 0)?,
        reviews: query_reviews(connection, app_id, community_id, None, 0)?,
    })
}

pub(super) fn export(
    connection: &Connection,
    app_id: &str,
    community_id: &str,
    offset: u32,
    limit: u16,
) -> Result<ToolkitCatalog> {
    let limit = i64::from(limit.min(500));
    let offset = i64::from(offset);
    Ok(ToolkitCatalog {
        concepts: query_concepts(connection, app_id, community_id, Some(limit), offset)?,
        tools: query_tools(connection, app_id, community_id, Some(limit), offset)?,
        assertions: query_assertions(connection, app_id, community_id, Some(limit), offset)?,
        reviews: query_reviews(connection, app_id, community_id, Some(limit), offset)?,
    })
}

fn query_concepts(
    connection: &Connection,
    app_id: &str,
    community_id: &str,
    limit: Option<i64>,
    offset: i64,
) -> Result<Vec<ToolkitConcept>> {
    let mut statement = connection.prepare(
        "SELECT concept_key,revision_hash,name,kind,definition,aliases_json,value_type,cardinality,applicable_json,allowed_values_json,scale_json,parent_key,status,active_revision,hex(source_operation_hash) \
         FROM toolkit_concepts WHERE app_id=?1 AND community_id=?2 ORDER BY concept_key,revision_hash LIMIT ?3 OFFSET ?4",
    )?;
    collect(statement.query_map(
        params![app_id, community_id, limit.unwrap_or(-1), offset],
        map_concept,
    )?)
}

fn query_tools(
    connection: &Connection,
    app_id: &str,
    community_id: &str,
    limit: Option<i64>,
    offset: i64,
) -> Result<Vec<ToolkitTool>> {
    let mut statement = connection.prepare(
        "SELECT tool_key,name,description,homepage,status,hex(source_operation_hash) FROM toolkit_tools \
         WHERE app_id=?1 AND community_id=?2 ORDER BY tool_key LIMIT ?3 OFFSET ?4",
    )?;
    let rows = statement.query_map(
        params![app_id, community_id, limit.unwrap_or(-1), offset],
        |row| {
            Ok(ToolkitTool {
                key: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                homepage: row.get(3)?,
                status: row.get(4)?,
                source_operation_hash: lower_hash(row.get(5)?),
            })
        },
    )?;
    collect(rows)
}

fn query_assertions(
    connection: &Connection,
    app_id: &str,
    community_id: &str,
    limit: Option<i64>,
    offset: i64,
) -> Result<Vec<ToolkitAssertion>> {
    let mut statement = connection.prepare(
        "SELECT assertion_id,tool_key,concept_key,concept_revision,value_json,origin,effective_state,source,evidence,as_of,rubric,rubric_version,rationale,evaluator_type,evaluation_date,hex(source_operation_hash),hex(review_operation_hash) \
         FROM toolkit_assertions WHERE app_id=?1 AND community_id=?2 ORDER BY assertion_id LIMIT ?3 OFFSET ?4",
    )?;
    let rows = statement.query_map(
        params![app_id, community_id, limit.unwrap_or(-1), offset],
        |row| {
            let value_json: Option<Vec<u8>> = row.get(4)?;
            Ok(ToolkitAssertion {
                assertion_id: row.get(0)?,
                tool: row.get(1)?,
                concept: row.get(2)?,
                concept_revision: row.get(3)?,
                value: value_json
                    .map(|bytes| parse_blob(&bytes))
                    .transpose()
                    .map_err(to_sql_error)?,
                origin: parse_enum(&row.get::<_, String>(5)?).map_err(to_sql_error)?,
                verification_state: parse_enum(&row.get::<_, String>(6)?).map_err(to_sql_error)?,
                source: row.get(7)?,
                evidence: row.get(8)?,
                as_of: row.get(9)?,
                rubric: row.get(10)?,
                rubric_version: row.get(11)?,
                rationale: row.get(12)?,
                evaluator_type: row.get(13)?,
                evaluation_date: row.get(14)?,
                source_operation_hash: lower_hash(row.get(15)?),
                review_operation_hash: row.get::<_, Option<String>>(16)?.map(lower_hash),
            })
        },
    )?;
    collect(rows)
}

fn query_reviews(
    connection: &Connection,
    app_id: &str,
    community_id: &str,
    limit: Option<i64>,
    offset: i64,
) -> Result<Vec<ToolkitReview>> {
    let mut statement = connection.prepare(
        "SELECT review_id,assertion_id,state,reviewer,rationale,review_date,hex(source_operation_hash) FROM toolkit_reviews \
         WHERE app_id=?1 AND community_id=?2 ORDER BY review_id LIMIT ?3 OFFSET ?4",
    )?;
    let rows = statement.query_map(
        params![app_id, community_id, limit.unwrap_or(-1), offset],
        |row| {
            Ok(ToolkitReview {
                review_id: row.get(0)?,
                assertion_id: row.get(1)?,
                state: parse_enum(&row.get::<_, String>(2)?).map_err(to_sql_error)?,
                reviewer: row.get(3)?,
                rationale: row.get(4)?,
                review_date: row.get(5)?,
                source_operation_hash: lower_hash(row.get(6)?),
            })
        },
    )?;
    collect(rows)
}

fn map_concept(row: &rusqlite::Row<'_>) -> rusqlite::Result<ToolkitConcept> {
    Ok(ToolkitConcept {
        definition: ConceptDefinition {
            key: row.get(0)?,
            name: row.get(2)?,
            kind: parse_enum(&row.get::<_, String>(3)?).map_err(to_sql_error)?,
            definition: row.get(4)?,
            aliases: parse_blob(&row.get::<_, Vec<u8>>(5)?).map_err(to_sql_error)?,
            value_type: parse_enum(&row.get::<_, String>(6)?).map_err(to_sql_error)?,
            cardinality: parse_enum(&row.get::<_, String>(7)?).map_err(to_sql_error)?,
            applicable_categories: parse_blob(&row.get::<_, Vec<u8>>(8)?).map_err(to_sql_error)?,
            allowed_values: row
                .get::<_, Option<Vec<u8>>>(9)?
                .map(|bytes| parse_blob(&bytes))
                .transpose()
                .map_err(to_sql_error)?,
            scale: row
                .get::<_, Option<Vec<u8>>>(10)?
                .map(|bytes| parse_blob(&bytes))
                .transpose()
                .map_err(to_sql_error)?,
            parent: row.get(11)?,
            status: parse_enum(&row.get::<_, String>(12)?).map_err(to_sql_error)?,
        },
        revision: row.get(1)?,
        active_revision: row.get::<_, i64>(13)? != 0,
        source_operation_hash: lower_hash(row.get(14)?),
    })
}

type TypedValueColumns = (&'static str, Option<f64>, Option<String>, Option<i64>);

fn typed_value(value: Option<&Value>) -> Result<TypedValueColumns> {
    match value {
        None | Some(Value::Null) => Ok(("none", None, None, None)),
        Some(Value::Bool(value)) => Ok(("boolean", None, None, Some(i64::from(*value)))),
        Some(Value::Number(value)) => Ok(("number", value.as_f64(), None, None)),
        Some(Value::String(value)) => Ok(("text", None, Some(value.clone()), None)),
        _ => Err(anyhow!("unsupported toolkit projection value")),
    }
}

fn enum_text<T: serde::Serialize>(value: &T) -> Result<String> {
    serde_json::to_value(value)?
        .as_str()
        .map(str::to_owned)
        .context("enum did not serialize as text")
}

fn parse_enum<T: DeserializeOwned>(value: &str) -> Result<T> {
    serde_json::from_value(Value::String(value.to_owned())).map_err(Into::into)
}

fn parse_blob<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes).context("toolkit projection JSON is corrupt")
}

fn lower_hash(value: String) -> String {
    value.to_ascii_lowercase()
}

fn to_sql_error(error: anyhow::Error) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Blob,
        Box::new(std::io::Error::other(error.to_string())),
    )
}

fn collect<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>>,
) -> Result<Vec<T>> {
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}
