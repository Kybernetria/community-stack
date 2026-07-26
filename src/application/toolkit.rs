use std::cmp::Ordering;

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::{Value, json};

use super::{CommunityCore, validate_idempotency_key, validate_object_id};
use crate::domain::{
    AddConcept, AddTool, AssertToolkitValue, AssertionOrigin, Cardinality, ConceptDefinition,
    ConceptKind, DocumentKey, ExplainToolkit, ExportToolkit, ListConcepts, Mutation,
    PrimitiveValue, ProjectionWrite, QueryOperator, QueryToolkit, RequirementExplanation,
    RequirementState, ShowConcept, ToolkitAssertion, ToolkitCatalog, ToolkitConcept,
    ToolkitProjection, ToolkitQueryPlan, ToolkitQueryResponse, ToolkitQueryResult,
    ToolkitRequirement, ToolkitReview, ToolkitTool, ValueType, VerificationState,
    VerifyToolkitAssertion,
};

const TOOLKIT_SCHEMA_VERSION: u32 = 1;
const MAX_CATALOG_ROWS: usize = 10_000;

impl CommunityCore {
    pub(super) async fn toolkit_schema_list(
        &self,
        app_id: &str,
        request: ListConcepts,
    ) -> Result<Value> {
        validate_object_id(&request.community_id, "community_id")?;
        if request.limit == 0 || request.limit > 500 {
            bail!("limit must be between 1 and 500");
        }
        Ok(serde_json::to_value(
            self.repository
                .toolkit_concepts(
                    app_id,
                    &request.community_id,
                    request.active_only,
                    request.limit,
                )
                .await?,
        )?)
    }

    pub(super) async fn toolkit_schema_show(
        &self,
        app_id: &str,
        request: ShowConcept,
    ) -> Result<Value> {
        validate_object_id(&request.community_id, "community_id")?;
        validate_key(&request.key, "concept key")?;
        let catalog = self
            .repository
            .toolkit_catalog(app_id, &request.community_id)
            .await?;
        let concept = catalog.concepts.into_iter().find(|concept| {
            concept.definition.key == request.key
                && request
                    .revision
                    .as_ref()
                    .is_none_or(|revision| concept.revision == *revision)
                && (request.revision.is_some() || concept.active_revision)
        });
        concept
            .map(serde_json::to_value)
            .transpose()?
            .context("concept revision was not found")
    }

    pub(super) async fn toolkit_schema_add(
        &self,
        app_id: &str,
        request: AddConcept,
    ) -> Result<Value> {
        validate_object_id(&request.community_id, "community_id")?;
        validate_idempotency_key(&request.idempotency_key)?;
        let request_hash = self.hash_request(&request)?;
        let mut definition = ConceptDefinition {
            key: request.key,
            name: request.name,
            kind: request.kind,
            definition: request.definition,
            aliases: request.aliases,
            value_type: request.value_type,
            cardinality: request.cardinality,
            applicable_categories: request.applicable_categories,
            allowed_values: request.allowed_values,
            scale: request.scale,
            parent: request.parent,
            status: request.status,
        };
        normalize_concept(&mut definition);
        validate_concept_shape(&definition)?;
        let lock_key = toolkit_lock_key(app_id, &request.community_id);
        let lock = self.document_lock(&lock_key).await;
        let _guard = lock.lock().await;
        let catalog = self
            .repository
            .toolkit_catalog(app_id, &request.community_id)
            .await?;
        validate_concept_references(&definition, &catalog)?;
        let canonical = serde_json::to_vec(&definition)?;
        let revision = hex::encode(self.hash_content(&canonical));
        if let Some(mut response) = self
            .repository
            .idempotency(app_id, &request.idempotency_key, request_hash)
            .await?
        {
            response["revision"] = json!(revision);
            return Ok(response);
        }
        if catalog
            .concepts
            .iter()
            .any(|item| item.definition.key == definition.key && item.revision == revision)
        {
            bail!("this immutable concept revision already exists");
        }
        let concept = ToolkitConcept {
            definition,
            revision: revision.clone(),
            source_operation_hash: String::new(),
            active_revision: true,
        };
        let document_id = format!("toolkit/concepts/{}/{revision}", concept.definition.key);
        let mut response = self
            .mutate(
                app_id,
                toolkit_mutation(
                    request.community_id,
                    document_id,
                    request.idempotency_key,
                    &concept,
                )?,
                request_hash,
                vec![ProjectionWrite::Toolkit(Box::new(
                    ToolkitProjection::Concept(concept),
                ))],
            )
            .await?;
        response["revision"] = json!(revision);
        Ok(response)
    }

    pub(super) async fn toolkit_tool_add(&self, app_id: &str, request: AddTool) -> Result<Value> {
        validate_object_id(&request.community_id, "community_id")?;
        validate_idempotency_key(&request.idempotency_key)?;
        validate_key(&request.key, "tool key")?;
        validate_text(&request.name, "name", 256)?;
        validate_optional_text(request.description.as_deref(), "description", 4_096)?;
        validate_optional_text(request.homepage.as_deref(), "homepage", 2_048)?;
        if !matches!(request.status.as_str(), "active" | "deprecated") {
            bail!("tool status must be active or deprecated");
        }
        let request_hash = self.hash_request(&request)?;
        if let Some(response) = self
            .repository
            .idempotency(app_id, &request.idempotency_key, request_hash)
            .await?
        {
            return Ok(response);
        }
        let lock_key = toolkit_lock_key(app_id, &request.community_id);
        let lock = self.document_lock(&lock_key).await;
        let _guard = lock.lock().await;
        let catalog = self
            .repository
            .toolkit_catalog(app_id, &request.community_id)
            .await?;
        if catalog.tools.iter().any(|tool| tool.key == request.key) {
            bail!("tool key already exists");
        }
        let tool = ToolkitTool {
            key: request.key,
            name: request.name,
            description: request.description,
            homepage: request.homepage,
            status: request.status,
            source_operation_hash: String::new(),
        };
        let document_id = format!("toolkit/tools/{}", tool.key);
        self.mutate(
            app_id,
            toolkit_mutation(
                request.community_id,
                document_id,
                request.idempotency_key,
                &tool,
            )?,
            request_hash,
            vec![ProjectionWrite::Toolkit(Box::new(ToolkitProjection::Tool(
                tool,
            )))],
        )
        .await
    }

    pub(super) async fn toolkit_assert(
        &self,
        app_id: &str,
        request: AssertToolkitValue,
    ) -> Result<Value> {
        validate_assert_request(&request)?;
        let request_hash = self.hash_request(&request)?;
        if let Some(mut response) = self
            .repository
            .idempotency(app_id, &request.idempotency_key, request_hash)
            .await?
        {
            response["assertion_id"] = json!(request.assertion_id);
            response["verification_state"] = json!(request.verification_state);
            return Ok(response);
        }
        let lock_key = toolkit_lock_key(app_id, &request.community_id);
        let lock = self.document_lock(&lock_key).await;
        let _guard = lock.lock().await;
        let catalog = self
            .repository
            .toolkit_catalog(app_id, &request.community_id)
            .await?;
        if catalog
            .assertions
            .iter()
            .any(|assertion| assertion.assertion_id == request.assertion_id)
        {
            bail!("assertion_id already exists");
        }
        let tool = catalog
            .tools
            .iter()
            .find(|tool| tool.key == request.tool)
            .context("tool was not found")?;
        if tool.status != "active" {
            bail!("assertions require an active tool");
        }
        let concept = resolve_concept(
            &catalog,
            &request.concept,
            request.concept_revision.as_deref(),
        )?;
        validate_assertion_value(concept, request.value.as_ref(), &request.verification_state)?;
        validate_assessment(concept, &request)?;
        validate_applicability(concept, &catalog, &request.tool)?;
        if request.verification_state == VerificationState::Verified {
            reject_local_cardinality_conflict(
                concept,
                &catalog,
                &request.tool,
                request.value.as_ref(),
                None,
            )?;
        }
        let assertion = ToolkitAssertion {
            assertion_id: request.assertion_id.clone(),
            tool: request.tool,
            concept: request.concept,
            concept_revision: concept.revision.clone(),
            value: request.value,
            origin: request.origin,
            verification_state: request.verification_state,
            source: request.source,
            evidence: request.evidence,
            as_of: request.as_of,
            rubric: request.rubric,
            rubric_version: request.rubric_version,
            rationale: request.rationale,
            evaluator_type: request.evaluator_type,
            evaluation_date: request.evaluation_date,
            source_operation_hash: String::new(),
            review_operation_hash: None,
        };
        let document_id = format!("toolkit/assertions/{}", request.assertion_id);
        let mut response = self
            .mutate(
                app_id,
                toolkit_mutation(
                    request.community_id,
                    document_id,
                    request.idempotency_key,
                    &assertion,
                )?,
                request_hash,
                vec![ProjectionWrite::Toolkit(Box::new(
                    ToolkitProjection::Assertion(assertion.clone()),
                ))],
            )
            .await?;
        response["assertion_id"] = json!(assertion.assertion_id);
        response["verification_state"] = json!(assertion.verification_state);
        Ok(response)
    }

    pub(super) async fn toolkit_verify(
        &self,
        app_id: &str,
        request: VerifyToolkitAssertion,
    ) -> Result<Value> {
        validate_object_id(&request.community_id, "community_id")?;
        validate_idempotency_key(&request.idempotency_key)?;
        validate_key(&request.review_id, "review_id")?;
        validate_key(&request.assertion_id, "assertion_id")?;
        validate_text(&request.reviewer, "reviewer", 256)?;
        validate_text(&request.rationale, "rationale", 8_192)?;
        validate_date(&request.review_date, "review_date")?;
        if !matches!(
            request.state,
            VerificationState::Verified | VerificationState::Rejected
        ) {
            bail!("a review state must be verified or rejected");
        }
        let request_hash = self.hash_request(&request)?;
        if let Some(mut response) = self
            .repository
            .idempotency(app_id, &request.idempotency_key, request_hash)
            .await?
        {
            response["review_id"] = json!(request.review_id);
            response["verification_state"] = json!(request.state);
            return Ok(response);
        }
        let lock_key = toolkit_lock_key(app_id, &request.community_id);
        let lock = self.document_lock(&lock_key).await;
        let _guard = lock.lock().await;
        let catalog = self
            .repository
            .toolkit_catalog(app_id, &request.community_id)
            .await?;
        if catalog
            .reviews
            .iter()
            .any(|review| review.review_id == request.review_id)
        {
            bail!("review_id already exists");
        }
        let assertion = catalog
            .assertions
            .iter()
            .find(|assertion| assertion.assertion_id == request.assertion_id)
            .context("assertion was not found")?;
        let concept = resolve_concept(
            &catalog,
            &assertion.concept,
            Some(&assertion.concept_revision),
        )?;
        validate_assertion_value(concept, assertion.value.as_ref(), &request.state)?;
        validate_applicability(concept, &catalog, &assertion.tool)?;
        if request.state == VerificationState::Verified {
            reject_local_cardinality_conflict(
                concept,
                &catalog,
                &assertion.tool,
                assertion.value.as_ref(),
                Some(&assertion.assertion_id),
            )?;
        }
        let review = ToolkitReview {
            review_id: request.review_id.clone(),
            assertion_id: request.assertion_id,
            state: request.state,
            reviewer: request.reviewer,
            rationale: request.rationale,
            review_date: request.review_date,
            source_operation_hash: String::new(),
        };
        let document_id = format!("toolkit/reviews/{}", request.review_id);
        let mut response = self
            .mutate(
                app_id,
                toolkit_mutation(
                    request.community_id,
                    document_id,
                    request.idempotency_key,
                    &review,
                )?,
                request_hash,
                vec![ProjectionWrite::Toolkit(Box::new(
                    ToolkitProjection::Review(review.clone()),
                ))],
            )
            .await?;
        response["review_id"] = json!(review.review_id);
        response["verification_state"] = json!(review.state);
        Ok(response)
    }

    pub(super) async fn toolkit_query(&self, app_id: &str, request: QueryToolkit) -> Result<Value> {
        validate_object_id(&request.community_id, "community_id")?;
        let catalog = self
            .repository
            .toolkit_catalog(app_id, &request.community_id)
            .await?;
        let response = evaluate_query(&catalog, request.plan)?;
        Ok(serde_json::to_value(response)?)
    }

    pub(super) async fn toolkit_explain(
        &self,
        app_id: &str,
        request: ExplainToolkit,
    ) -> Result<Value> {
        validate_object_id(&request.community_id, "community_id")?;
        validate_key(&request.tool, "tool")?;
        let catalog = self
            .repository
            .toolkit_catalog(app_id, &request.community_id)
            .await?;
        let mut plan = request.query;
        plan.tools = vec![request.tool];
        let result = evaluate_query(&catalog, plan)?
            .results
            .into_iter()
            .next()
            .context("tool was not found")?;
        Ok(serde_json::to_value(result)?)
    }

    pub(super) async fn toolkit_export(
        &self,
        app_id: &str,
        request: ExportToolkit,
    ) -> Result<Value> {
        validate_object_id(&request.community_id, "community_id")?;
        if request.limit == 0 || request.limit > 500 {
            bail!("export limit must be between 1 and 500");
        }
        let catalog = self
            .repository
            .toolkit_export(app_id, &request.community_id, request.offset, request.limit)
            .await?;
        Ok(json!({
            "format": "community-stack-toolkit-export",
            "version": 1,
            "community_id": request.community_id,
            "offset": request.offset,
            "limit": request.limit,
            "concepts": catalog.concepts,
            "tools": catalog.tools,
            "assertions": catalog.assertions,
            "reviews": catalog.reviews,
            "import_supported": false
        }))
    }
}

fn toolkit_lock_key(app_id: &str, community_id: &str) -> DocumentKey {
    DocumentKey {
        app_id: app_id.into(),
        community_id: community_id.into(),
        document_id: "toolkit/__local-write-lock".into(),
    }
}

fn toolkit_mutation<T: Serialize>(
    community_id: String,
    document_id: String,
    idempotency_key: String,
    record: &T,
) -> Result<crate::domain::MutateDocument> {
    let canonical = serde_json::to_string(record)?;
    Ok(crate::domain::MutateDocument {
        community_id,
        document_id,
        idempotency_key,
        schema_version: TOOLKIT_SCHEMA_VERSION,
        mutations: vec![Mutation::MapSet {
            container: "record".into(),
            key: "canonical_json".into(),
            value: PrimitiveValue::String(canonical),
        }],
    })
}

fn normalize_concept(concept: &mut ConceptDefinition) {
    concept.aliases.sort();
    concept.aliases.dedup();
    concept.applicable_categories.sort();
    concept.applicable_categories.dedup();
    if let Some(values) = &mut concept.allowed_values {
        values.sort();
        values.dedup();
    }
}

fn validate_concept_shape(concept: &ConceptDefinition) -> Result<()> {
    validate_key(&concept.key, "concept key")?;
    if !concept.key.contains('.') {
        bail!("concept key must be namespaced");
    }
    validate_text(&concept.name, "name", 256)?;
    validate_text(&concept.definition, "definition", 8_192)?;
    for alias in &concept.aliases {
        validate_text(alias, "alias", 256)?;
    }
    for category in &concept.applicable_categories {
        validate_key(category, "applicable category")?;
    }
    if let Some(parent) = &concept.parent {
        validate_key(parent, "parent")?;
        if parent == &concept.key {
            bail!("a concept cannot be its own parent");
        }
    }
    match concept.value_type {
        ValueType::Enum => {
            let values = concept
                .allowed_values
                .as_ref()
                .filter(|values| !values.is_empty())
                .context("enum concepts require allowed_values")?;
            for value in values {
                validate_text(value, "allowed value", 256)?;
            }
        }
        _ if concept.allowed_values.is_some() => {
            bail!("allowed_values are valid only for enum concepts")
        }
        _ => {}
    }
    if concept.kind == ConceptKind::Dimension {
        let scale = concept
            .scale
            .as_ref()
            .context("dimensions require a scale")?;
        if !scale.min.is_finite() || !scale.max.is_finite() || scale.min > scale.max {
            bail!("dimension scale must have finite ordered bounds");
        }
        validate_key(&scale.rubric, "rubric")?;
        validate_text(&scale.version, "rubric version", 64)?;
        if scale.anchors.is_empty() {
            bail!("dimension scale requires anchors");
        }
        if !matches!(concept.value_type, ValueType::Integer | ValueType::Number) {
            bail!("dimensions require an integer or number value type");
        }
    } else if concept.scale.is_some() {
        bail!("scale is valid only for dimension concepts");
    }
    Ok(())
}

fn validate_concept_references(
    concept: &ConceptDefinition,
    catalog: &ToolkitCatalog,
) -> Result<()> {
    for reference in concept
        .applicable_categories
        .iter()
        .chain(concept.parent.iter())
    {
        let target = resolve_concept(catalog, reference, None)
            .with_context(|| format!("referenced concept {reference} was not found"))?;
        if concept.applicable_categories.contains(reference)
            && !matches!(
                target.definition.kind,
                ConceptKind::Domain | ConceptKind::Role
            )
        {
            bail!("applicable_categories must reference domain or role concepts");
        }
    }
    Ok(())
}

fn resolve_concept<'a>(
    catalog: &'a ToolkitCatalog,
    key: &str,
    revision: Option<&str>,
) -> Result<&'a ToolkitConcept> {
    catalog
        .concepts
        .iter()
        .find(|concept| {
            concept.definition.key == key
                && revision.is_none_or(|revision| concept.revision == revision)
                && (revision.is_some() || concept.active_revision)
        })
        .context("concept revision was not found")
}

fn validate_assert_request(request: &AssertToolkitValue) -> Result<()> {
    validate_object_id(&request.community_id, "community_id")?;
    validate_idempotency_key(&request.idempotency_key)?;
    validate_key(&request.assertion_id, "assertion_id")?;
    validate_key(&request.tool, "tool")?;
    validate_key(&request.concept, "concept")?;
    validate_text(&request.source, "source", 2_048)?;
    validate_text(&request.evidence, "evidence", 8_192)?;
    validate_date(&request.as_of, "as_of")?;
    if request.origin == AssertionOrigin::Ai
        && request.verification_state != VerificationState::Proposed
    {
        bail!("AI assertions must begin in proposed state");
    }
    Ok(())
}

fn validate_assertion_value(
    concept: &ToolkitConcept,
    value: Option<&Value>,
    state: &VerificationState,
) -> Result<()> {
    if *state == VerificationState::Unknown {
        if value.is_some_and(|value| !value.is_null()) {
            bail!("unknown assertions must not carry a value");
        }
        return Ok(());
    }
    let value = value.filter(|value| !value.is_null()).context(
        "proposed, verified, and rejected assertions require an explicit non-null value",
    )?;
    match concept.definition.value_type {
        ValueType::Boolean if !value.is_boolean() => bail!("value_type_mismatch: expected boolean"),
        ValueType::Integer if value.as_i64().is_none() => {
            bail!("value_type_mismatch: expected signed integer")
        }
        ValueType::Number if value.as_f64().is_none() => {
            bail!("value_type_mismatch: expected finite number")
        }
        ValueType::Text | ValueType::Date | ValueType::Reference | ValueType::Enum
            if !value.is_string() =>
        {
            bail!("value_type_mismatch: expected string")
        }
        _ => {}
    }
    if let Some(number) = value.as_f64() {
        if !number.is_finite() {
            bail!("numeric values must be finite");
        }
        if let Some(scale) = &concept.definition.scale
            && !(scale.min..=scale.max).contains(&number)
        {
            bail!("value_out_of_range: score must be within the concept scale");
        }
    }
    if concept.definition.value_type == ValueType::Enum
        && !concept
            .definition
            .allowed_values
            .as_ref()
            .is_some_and(|allowed| {
                allowed
                    .iter()
                    .any(|item| Some(item.as_str()) == value.as_str())
            })
    {
        bail!("value_not_allowed: enum value is outside allowed_values");
    }
    if concept.definition.value_type == ValueType::Date {
        validate_date(value.as_str().unwrap_or_default(), "value")?;
    }
    Ok(())
}

fn validate_assessment(concept: &ToolkitConcept, request: &AssertToolkitValue) -> Result<()> {
    let fields_present = request.rubric.is_some()
        || request.rubric_version.is_some()
        || request.rationale.is_some()
        || request.evaluator_type.is_some()
        || request.evaluation_date.is_some();
    if concept.definition.kind != ConceptKind::Dimension {
        if fields_present {
            bail!("assessment metadata is valid only for dimensions");
        }
        return Ok(());
    }
    let scale = concept.definition.scale.as_ref().unwrap();
    if request.rubric.as_deref() != Some(&scale.rubric)
        || request.rubric_version.as_deref() != Some(&scale.version)
        || request.rationale.as_deref().is_none_or(str::is_empty)
        || request.evaluator_type.as_deref().is_none_or(str::is_empty)
        || request.evaluation_date.as_deref().is_none_or(str::is_empty)
    {
        bail!(
            "incomplete_assessment: rubric, version, rationale, evaluator_type, and evaluation_date are required"
        );
    }
    validate_date(
        request.evaluation_date.as_deref().unwrap_or_default(),
        "evaluation_date",
    )?;
    if request.evaluator_type.as_deref() == Some("ai")
        && request.verification_state != VerificationState::Proposed
    {
        bail!("AI-only evaluations must begin in proposed state");
    }
    Ok(())
}

fn validate_applicability(
    concept: &ToolkitConcept,
    catalog: &ToolkitCatalog,
    tool: &str,
) -> Result<()> {
    if concept.definition.applicable_categories.is_empty() {
        return Ok(());
    }
    let applicable = concept
        .definition
        .applicable_categories
        .iter()
        .any(|category| {
            resolve_concept(catalog, category, None).is_ok_and(|category_concept| {
                catalog.assertions.iter().any(|assertion| {
                    assertion.tool == tool
                        && assertion.concept == *category
                        && assertion.concept_revision == category_concept.revision
                        && assertion.verification_state == VerificationState::Verified
                        && assertion.value == Some(Value::Bool(true))
                })
            })
        });
    if !applicable {
        bail!("concept_not_applicable: tool lacks a verified applicable category");
    }
    Ok(())
}

fn reject_local_cardinality_conflict(
    concept: &ToolkitConcept,
    catalog: &ToolkitCatalog,
    tool: &str,
    value: Option<&Value>,
    except_assertion: Option<&str>,
) -> Result<()> {
    if concept.definition.cardinality == Cardinality::Many {
        return Ok(());
    }
    if catalog.assertions.iter().any(|assertion| {
        assertion.tool == tool
            && assertion.concept == concept.definition.key
            && assertion.concept_revision == concept.revision
            && assertion.verification_state == VerificationState::Verified
            && Some(assertion.assertion_id.as_str()) != except_assertion
            && assertion.value.as_ref() != value
    }) {
        bail!("cardinality_conflict: a different verified value is already visible");
    }
    Ok(())
}

fn normalized_requirements(plan: &ToolkitQueryPlan) -> Result<Vec<ToolkitRequirement>> {
    let mut requirements = plan.requirements.clone();
    requirements.extend(plan.mandatory.iter().cloned().map(|mut requirement| {
        requirement.required = true;
        requirement
    }));
    requirements.extend(plan.optional.iter().cloned().map(|mut requirement| {
        requirement.required = false;
        requirement
    }));
    if requirements.is_empty() || requirements.len() > 64 {
        bail!("query must contain 1..=64 requirements");
    }
    Ok(requirements)
}

fn evaluate_query(
    catalog: &ToolkitCatalog,
    plan: ToolkitQueryPlan,
) -> Result<ToolkitQueryResponse> {
    if catalog.concepts.len() + catalog.tools.len() + catalog.assertions.len() > MAX_CATALOG_ROWS {
        bail!("toolkit catalog exceeds the bounded local query limit");
    }
    if plan.tools.len() > 500 {
        bail!("tool restriction exceeds 500 entries");
    }
    for tool in &plan.tools {
        validate_key(tool, "tool restriction")?;
    }
    let requirements = normalized_requirements(&plan)?;
    let resolved = requirements
        .into_iter()
        .map(|requirement| {
            validate_key(&requirement.concept, "requirement concept")?;
            let concept = resolve_concept(
                catalog,
                &requirement.concept,
                requirement.concept_revision.as_deref(),
            )?;
            validate_requirement(concept, &requirement)?;
            Ok((requirement, concept))
        })
        .collect::<Result<Vec<_>>>()?;

    let mut results = Vec::new();
    for tool in catalog.tools.iter().filter(|tool| {
        tool.status == "active" && (plan.tools.is_empty() || plan.tools.contains(&tool.key))
    }) {
        let explanations = resolved
            .iter()
            .map(|(requirement, concept)| {
                evaluate_requirement(catalog, &tool.key, requirement, concept)
            })
            .collect::<Vec<_>>();
        let exact = explanations
            .iter()
            .filter(|item| item.required)
            .all(|item| item.state == RequirementState::Satisfied);
        let satisfied = explanations
            .iter()
            .filter(|item| item.state == RequirementState::Satisfied)
            .count();
        if exact || plan.include_partial {
            results.push(ToolkitQueryResult {
                tool: tool.key.clone(),
                name: tool.name.clone(),
                match_state: if exact { "exact" } else { "partial" }.into(),
                satisfied_requirements: satisfied,
                requirements: explanations,
            });
        }
    }
    results.sort_by(|left, right| {
        let left_exact = left.match_state == "exact";
        let right_exact = right.match_state == "exact";
        right_exact
            .cmp(&left_exact)
            .then_with(|| {
                right
                    .satisfied_requirements
                    .cmp(&left.satisfied_requirements)
            })
            .then_with(|| left.tool.cmp(&right.tool))
    });
    let exact_count = results
        .iter()
        .filter(|result| result.match_state == "exact")
        .count();
    Ok(ToolkitQueryResponse {
        exact_count,
        results,
    })
}

fn validate_requirement(concept: &ToolkitConcept, requirement: &ToolkitRequirement) -> Result<()> {
    match requirement.op {
        QueryOperator::Exists => {
            if requirement.value.is_some() {
                bail!("exists does not accept a value");
            }
        }
        QueryOperator::In => {
            let values = requirement
                .value
                .as_ref()
                .and_then(Value::as_array)
                .filter(|values| !values.is_empty())
                .context("in requires a non-empty value array")?;
            for value in values {
                validate_assertion_value(concept, Some(value), &VerificationState::Verified)?;
            }
        }
        QueryOperator::Gt | QueryOperator::Gte | QueryOperator::Lt | QueryOperator::Lte => {
            if !matches!(
                concept.definition.value_type,
                ValueType::Integer | ValueType::Number
            ) {
                bail!("range operators require a numeric concept");
            }
            validate_assertion_value(
                concept,
                requirement.value.as_ref(),
                &VerificationState::Verified,
            )?;
        }
        QueryOperator::Eq | QueryOperator::Ne => validate_assertion_value(
            concept,
            requirement.value.as_ref(),
            &VerificationState::Verified,
        )?,
    }
    Ok(())
}

fn evaluate_requirement(
    catalog: &ToolkitCatalog,
    tool: &str,
    requirement: &ToolkitRequirement,
    concept: &ToolkitConcept,
) -> RequirementExplanation {
    let assertions = catalog
        .assertions
        .iter()
        .filter(|assertion| {
            assertion.tool == tool
                && assertion.concept == requirement.concept
                && assertion.concept_revision == concept.revision
        })
        .cloned()
        .collect::<Vec<_>>();
    let verified = assertions
        .iter()
        .filter(|assertion| assertion.verification_state == VerificationState::Verified)
        .collect::<Vec<_>>();
    let distinct = verified
        .iter()
        .fold(Vec::<&Value>::new(), |mut values, assertion| {
            if let Some(value) = assertion.value.as_ref()
                && !values.contains(&value)
            {
                values.push(value);
            }
            values
        });
    let state = if concept.definition.cardinality == Cardinality::One && distinct.len() > 1 {
        RequirementState::Conflict
    } else if !verified.is_empty() {
        if requirement_matches(&verified, requirement) {
            RequirementState::Satisfied
        } else {
            RequirementState::Unsatisfied
        }
    } else if assertions
        .iter()
        .any(|item| item.verification_state == VerificationState::Rejected)
    {
        RequirementState::Rejected
    } else if assertions
        .iter()
        .any(|item| item.verification_state == VerificationState::Proposed)
    {
        RequirementState::Proposed
    } else if assertions
        .iter()
        .any(|item| item.verification_state == VerificationState::Unknown)
    {
        RequirementState::Unknown
    } else {
        RequirementState::Missing
    };
    RequirementExplanation {
        concept: requirement.concept.clone(),
        concept_revision: concept.revision.clone(),
        op: requirement.op.clone(),
        required: requirement.required,
        state,
        assertions,
    }
}

fn requirement_matches(assertions: &[&ToolkitAssertion], requirement: &ToolkitRequirement) -> bool {
    match requirement.op {
        QueryOperator::Exists => true,
        QueryOperator::Ne => assertions
            .iter()
            .all(|assertion| assertion.value.as_ref() != requirement.value.as_ref()),
        _ => assertions.iter().any(|assertion| {
            let Some(value) = assertion.value.as_ref() else {
                return false;
            };
            match requirement.op {
                QueryOperator::Eq => Some(value) == requirement.value.as_ref(),
                QueryOperator::In => requirement
                    .value
                    .as_ref()
                    .and_then(Value::as_array)
                    .is_some_and(|values| values.contains(value)),
                QueryOperator::Gt => {
                    numeric_cmp(value, requirement.value.as_ref()) == Some(Ordering::Greater)
                }
                QueryOperator::Gte => matches!(
                    numeric_cmp(value, requirement.value.as_ref()),
                    Some(Ordering::Greater | Ordering::Equal)
                ),
                QueryOperator::Lt => {
                    numeric_cmp(value, requirement.value.as_ref()) == Some(Ordering::Less)
                }
                QueryOperator::Lte => matches!(
                    numeric_cmp(value, requirement.value.as_ref()),
                    Some(Ordering::Less | Ordering::Equal)
                ),
                QueryOperator::Exists | QueryOperator::Ne => false,
            }
        }),
    }
}

fn numeric_cmp(left: &Value, right: Option<&Value>) -> Option<Ordering> {
    left.as_f64()?.partial_cmp(&right?.as_f64()?)
}

fn validate_key(value: &str, field: &str) -> Result<()> {
    if value.is_empty() || value.len() > 128 {
        bail!("{field} must contain 1..=128 bytes");
    }
    if !value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':' | b'/')
    }) {
        bail!("{field} contains unsupported characters");
    }
    Ok(())
}

fn validate_text(value: &str, field: &str, max: usize) -> Result<()> {
    if value.is_empty() || value.len() > max || value.chars().any(char::is_control) {
        bail!("{field} must contain 1..={max} printable bytes");
    }
    Ok(())
}

fn validate_optional_text(value: Option<&str>, field: &str, max: usize) -> Result<()> {
    if let Some(value) = value {
        validate_text(value, field, max)?;
    }
    Ok(())
}

fn validate_date(value: &str, field: &str) -> Result<()> {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes
            .iter()
            .enumerate()
            .any(|(index, byte)| index != 4 && index != 7 && !byte.is_ascii_digit())
    {
        bail!("{field} must use YYYY-MM-DD");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conflict_catalog() -> ToolkitCatalog {
        let definition = ConceptDefinition {
            key: "attr.test.value".into(),
            name: "Value".into(),
            kind: ConceptKind::Attribute,
            definition: "A cardinality-one value.".into(),
            aliases: vec![],
            value_type: ValueType::Text,
            cardinality: Cardinality::One,
            applicable_categories: vec![],
            allowed_values: None,
            scale: None,
            parent: None,
            status: crate::domain::ConceptStatus::Active,
        };
        let concept_key = definition.key.clone();
        let assertion = |id: &str, value: &str, hash: &str| ToolkitAssertion {
            assertion_id: id.into(),
            tool: "tool".into(),
            concept: concept_key.clone(),
            concept_revision: "r".repeat(64),
            value: Some(json!(value)),
            origin: AssertionOrigin::Human,
            verification_state: VerificationState::Verified,
            source: "https://example.test".into(),
            evidence: format!("evidence-{id}"),
            as_of: "2026-07-26".into(),
            rubric: None,
            rubric_version: None,
            rationale: None,
            evaluator_type: None,
            evaluation_date: None,
            source_operation_hash: hash.repeat(64),
            review_operation_hash: None,
        };
        ToolkitCatalog {
            concepts: vec![ToolkitConcept {
                definition,
                revision: "r".repeat(64),
                source_operation_hash: "c".repeat(64),
                active_revision: true,
            }],
            tools: vec![ToolkitTool {
                key: "tool".into(),
                name: "Tool".into(),
                description: None,
                homepage: None,
                status: "active".into(),
                source_operation_hash: "t".repeat(64),
            }],
            assertions: vec![
                assertion("signed-a", "one", "a"),
                assertion("signed-b", "two", "b"),
            ],
            reviews: vec![],
        }
    }

    #[test]
    fn replicated_cardinality_one_conflict_fails_closed_and_names_signed_records() {
        let response = evaluate_query(
            &conflict_catalog(),
            ToolkitQueryPlan {
                tools: vec!["tool".into()],
                requirements: vec![ToolkitRequirement {
                    concept: "attr.test.value".into(),
                    concept_revision: None,
                    op: QueryOperator::Eq,
                    value: Some(json!("one")),
                    required: true,
                }],
                mandatory: vec![],
                optional: vec![],
                include_partial: true,
            },
        )
        .unwrap();
        assert_eq!(response.exact_count, 0);
        let explanation = &response.results[0].requirements[0];
        assert_eq!(explanation.state, RequirementState::Conflict);
        assert_eq!(
            explanation
                .assertions
                .iter()
                .map(|item| item.assertion_id.as_str())
                .collect::<Vec<_>>(),
            ["signed-a", "signed-b"]
        );
        assert!(
            explanation.assertions.iter().all(|item| {
                item.source_operation_hash.len() == 64 && !item.evidence.is_empty()
            })
        );
    }
}
