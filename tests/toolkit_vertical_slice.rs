#![allow(clippy::too_many_lines)]

use std::{path::Path, sync::Arc};

use base64::Engine;
use community_stack::{
    adapters::{
        hashing::Blake3ContentHasher,
        loro::LoroDocumentEngine,
        p2panda::{P2pandaSecureLog, decode_update},
        sqlite::{self, StoreHandle},
    },
    application::CommunityCore,
    config,
    domain::Principal,
};
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use tempfile::TempDir;

fn core(db: &Path, data: &Path, peer: u64) -> (CommunityCore, [u8; 32]) {
    let (signing_key, master_key) = config::load_keys(data).unwrap();
    (
        CommunityCore::new(
            Arc::new(StoreHandle::start(db).unwrap()),
            Arc::new(LoroDocumentEngine),
            Arc::new(P2pandaSecureLog::new(signing_key, master_key)),
            Arc::new(Blake3ContentHasher),
            peer,
        ),
        master_key,
    )
}

async fn setup(app_id: &str) -> (TempDir, CommunityCore, Principal, String, String, [u8; 32]) {
    let temp = TempDir::new().unwrap();
    config::initialize_data_dir(temp.path()).unwrap();
    let db = config::database_path(temp.path());
    let (hash, token) = config::generate_token().unwrap();
    sqlite::register_application(&db, app_id, &hash, "APP").unwrap();
    let (transport_hash, transport_token) = config::generate_token().unwrap();
    sqlite::register_application(&db, "test-transport", &transport_hash, "TRANSPORT").unwrap();
    let (scope_hash, scope_token) = config::generate_token().unwrap();
    sqlite::register_application(&db, "org.test.scope-b", &scope_hash, "APP").unwrap();
    let (core, master_key) = core(&db, temp.path(), 91);
    let principal = core.authenticate(&token).await.unwrap().unwrap();
    (
        temp,
        core,
        principal,
        transport_token,
        scope_token,
        master_key,
    )
}

async fn call(core: &CommunityCore, app: &Principal, method: &str, params: Value) -> Value {
    core.call_authenticated(app, method, params).await.unwrap()
}

fn bool_concept(key: &str) -> Value {
    json!({
        "key": key, "name": key, "kind": "attribute", "definition": "Test definition",
        "aliases": [], "value_type": "boolean", "cardinality": "one",
        "applicable_categories": [], "allowed_values": null, "scale": null,
        "parent": null, "status": "active"
    })
}

async fn add_concept(
    core: &CommunityCore,
    app: &Principal,
    community: &str,
    id: &str,
    definition: Value,
) -> Value {
    let mut object = definition.as_object().unwrap().clone();
    object.insert("community_id".into(), json!(community));
    object.insert("idempotency_key".into(), json!(format!("concept-{id}")));
    call(core, app, "toolkit.schema.add", Value::Object(object)).await
}

async fn add_tool(core: &CommunityCore, app: &Principal, community: &str, key: &str) {
    call(
        core,
        app,
        "toolkit.tool.add",
        json!({
            "community_id": community, "idempotency_key": format!("tool-{key}"),
            "key": key, "name": key
        }),
    )
    .await;
}

fn assertion(
    community: &str,
    id: &str,
    tool: &str,
    concept: &str,
    value: Value,
    state: &str,
) -> Value {
    json!({
        "community_id": community, "idempotency_key": format!("assert-{id}"),
        "assertion_id": id, "tool": tool, "concept": concept, "value": value,
        "origin": "human", "verification_state": state,
        "source": "https://example.test/source", "evidence": "Bounded test evidence.",
        "as_of": "2026-07-26"
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn toolkit_write_is_atomic_secure_idempotent_projected_and_reconstructable() {
    let (temp, core, app, transport_token, _, master_key) = setup("org.test.toolkit").await;
    let db = config::database_path(temp.path());
    let first = add_concept(
        &core,
        &app,
        "catalog",
        "feature",
        bool_concept("cap.test.feature"),
    )
    .await;
    let retry = add_concept(
        &core,
        &app,
        "catalog",
        "feature",
        bool_concept("cap.test.feature"),
    )
    .await;
    assert_eq!(first, retry);
    add_tool(&core, &app, "catalog", "tool-a").await;
    let proposed = call(
        &core,
        &app,
        "toolkit.assert",
        json!({
            "community_id": "catalog", "idempotency_key": "ai-assert",
            "assertion_id": "a-ai", "tool": "tool-a", "concept": "cap.test.feature",
            "value": true, "origin": "ai", "verification_state": "proposed",
            "source": "https://example.test/ai", "evidence": "AI proposal pending review.",
            "as_of": "2026-07-26"
        }),
    )
    .await;
    assert_eq!(proposed["verification_state"], "proposed");

    let before = call(
        &core,
        &app,
        "toolkit.query",
        json!({"community_id":"catalog","tools":["tool-a"],"requirements":[
            {"concept":"cap.test.feature","op":"eq","value":true}
        ]}),
    )
    .await;
    assert_eq!(before["exact_count"], 0);
    assert_eq!(before["results"][0]["requirements"][0]["state"], "proposed");

    call(
        &core,
        &app,
        "toolkit.verify",
        json!({
            "community_id":"catalog", "idempotency_key":"review-ai", "review_id":"review-ai",
            "assertion_id":"a-ai", "state":"verified", "reviewer":"local-app-reviewer",
            "rationale":"Source reviewed explicitly.", "review_date":"2026-07-26"
        }),
    )
    .await;
    let after = call(
        &core,
        &app,
        "toolkit.query",
        json!({"community_id":"catalog","tools":["tool-a"],"requirements":[
            {"concept":"cap.test.feature","op":"eq","value":true}
        ]}),
    )
    .await;
    assert_eq!(after["exact_count"], 1);
    let asserted_hash =
        after["results"][0]["requirements"][0]["assertions"][0]["source_operation_hash"]
            .as_str()
            .unwrap();
    assert_eq!(asserted_hash.len(), 64);

    let document = call(
        &core,
        &app,
        "document.get",
        json!({"community_id":"catalog","document_id":"toolkit/assertions/a-ai"}),
    )
    .await;
    let authoritative: Value = serde_json::from_str(
        document["state"]["record"]["canonical_json"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(authoritative["assertion_id"], "a-ai");
    assert_eq!(authoritative["concept_revision"], first["revision"]);

    let reader = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let linked: i64 = reader
        .query_row(
            "SELECT count(*) FROM toolkit_assertions a JOIN operations o ON o.operation_hash=a.source_operation_hash JOIN durable_outbox q ON q.operation_hash=o.operation_hash WHERE a.assertion_id='a-ai'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(linked, 1);
    let idempotency: i64 = reader
        .query_row(
            "SELECT count(*) FROM idempotency_keys WHERE key='ai-assert'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(idempotency, 1);
    drop(reader);

    let transport = core.authenticate(&transport_token).await.unwrap().unwrap();
    let claimed = call(
        &core,
        &transport,
        "outbox.claim",
        json!({"transport":"test-transport","limit":16,"max_bytes":1_048_576}),
    )
    .await;
    let item = claimed
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["operation_hash"] == asserted_hash)
        .unwrap();
    let header = base64::engine::general_purpose::STANDARD
        .decode(item["header_base64"].as_str().unwrap())
        .unwrap();
    let body = base64::engine::general_purpose::STANDARD
        .decode(item["body_base64"].as_str().unwrap())
        .unwrap();
    let (header, payload) = decode_update(&header, &body, &master_key).unwrap();
    assert_eq!(header.extensions.document_id, "toolkit/assertions/a-ai");
    assert!(!payload.loro_update.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn validation_and_query_states_fail_closed() {
    let (_temp, core, app, _, _, _) = setup("org.test.validation").await;
    let community = "validation";
    add_concept(
        &core,
        &app,
        community,
        "bool",
        bool_concept("attr.test.bool"),
    )
    .await;
    add_concept(
        &core,
        &app,
        community,
        "enum",
        json!({
            "key":"attr.test.transport","name":"Transport","kind":"attribute","definition":"Transport",
            "aliases":[],"value_type":"enum","cardinality":"many","applicable_categories":[],
            "allowed_values":["tcp","udp","iroh"],"scale":null,"parent":null,"status":"active"
        }),
    )
    .await;
    add_concept(
        &core,
        &app,
        community,
        "number",
        json!({
            "key":"attr.test.score","name":"Score","kind":"attribute","definition":"Numeric score",
            "aliases":[],"value_type":"number","cardinality":"one","applicable_categories":[],
            "allowed_values":null,"scale":null,"parent":null,"status":"active"
        }),
    )
    .await;
    add_tool(&core, &app, community, "alpha").await;
    add_tool(&core, &app, community, "beta").await;

    call(
        &core,
        &app,
        "toolkit.assert",
        assertion(
            community,
            "false",
            "alpha",
            "attr.test.bool",
            json!(false),
            "verified",
        ),
    )
    .await;
    call(
        &core,
        &app,
        "toolkit.assert",
        assertion(
            community,
            "zero",
            "alpha",
            "attr.test.score",
            json!(0),
            "verified",
        ),
    )
    .await;
    call(
        &core,
        &app,
        "toolkit.assert",
        assertion(
            community,
            "tcp",
            "alpha",
            "attr.test.transport",
            json!("tcp"),
            "verified",
        ),
    )
    .await;
    call(
        &core,
        &app,
        "toolkit.assert",
        assertion(
            community,
            "udp",
            "alpha",
            "attr.test.transport",
            json!("udp"),
            "verified",
        ),
    )
    .await;
    call(
        &core,
        &app,
        "toolkit.assert",
        assertion(
            community,
            "unknown",
            "beta",
            "attr.test.bool",
            Value::Null,
            "unknown",
        ),
    )
    .await;
    call(
        &core,
        &app,
        "toolkit.assert",
        assertion(
            community,
            "rejected",
            "beta",
            "attr.test.score",
            json!(5),
            "rejected",
        ),
    )
    .await;

    let query = call(
        &core,
        &app,
        "toolkit.query",
        json!({"community_id":community,"tools":["alpha"],"requirements":[
            {"concept":"attr.test.bool","op":"eq","value":true},
            {"concept":"attr.test.score","op":"gte","value":0},
            {"concept":"attr.test.score","op":"lt","value":1},
            {"concept":"attr.test.transport","op":"ne","value":"iroh"}
        ]}),
    )
    .await;
    assert_eq!(query["exact_count"], 0); // mandatory conjunction: verified false fails.
    assert_eq!(
        query["results"][0]["requirements"][0]["state"],
        "unsatisfied"
    );
    assert_eq!(query["results"][0]["requirements"][1]["state"], "satisfied");
    assert_eq!(query["results"][0]["requirements"][2]["state"], "satisfied");
    assert_eq!(query["results"][0]["requirements"][3]["state"], "satisfied");

    let universal_ne = call(
        &core,
        &app,
        "toolkit.query",
        json!({"community_id":community,"tools":["alpha"],"requirements":[
            {"concept":"attr.test.transport","op":"ne","value":"tcp"}
        ]}),
    )
    .await;
    assert_eq!(universal_ne["exact_count"], 0);
    assert_eq!(
        universal_ne["results"][0]["requirements"][0]["state"],
        "unsatisfied"
    );

    let states = call(
        &core,
        &app,
        "toolkit.query",
        json!({"community_id":community,"tools":["beta"],"requirements":[
            {"concept":"attr.test.bool","op":"eq","value":false},
            {"concept":"attr.test.score","op":"eq","value":5},
            {"concept":"attr.test.transport","op":"exists"}
        ]}),
    )
    .await;
    assert_eq!(states["results"][0]["requirements"][0]["state"], "unknown");
    assert_eq!(states["results"][0]["requirements"][1]["state"], "rejected");
    assert_eq!(states["results"][0]["requirements"][2]["state"], "missing");

    let bad_type = core
        .call_authenticated(
            &app,
            "toolkit.assert",
            assertion(
                community,
                "bad-type",
                "alpha",
                "attr.test.score",
                json!("high"),
                "verified",
            ),
        )
        .await
        .unwrap_err();
    assert!(bad_type.to_string().contains("value_type_mismatch"));
    let bad_enum = core
        .call_authenticated(
            &app,
            "toolkit.assert",
            assertion(
                community,
                "bad-enum",
                "alpha",
                "attr.test.transport",
                json!("sql' OR 1=1--"),
                "verified",
            ),
        )
        .await
        .unwrap_err();
    assert!(bad_enum.to_string().contains("value_not_allowed"));
    let injection = core
        .call_authenticated(
            &app,
            "toolkit.query",
            json!({"community_id":community,"requirements":[{"concept":"x' OR 1=1--","op":"exists"}]}),
        )
        .await
        .unwrap_err();
    assert!(injection.to_string().contains("unsupported characters"));
}

#[tokio::test(flavor = "multi_thread")]
async fn applicability_assessment_and_namespaces_are_enforced() {
    let (_temp, core, app, _, token_b, _) = setup("org.test.scope-a").await;
    let community = "scope";
    add_concept(
        &core,
        &app,
        community,
        "domain",
        json!({"key":"domain.test","name":"Test domain","kind":"domain","definition":"Domain",
            "aliases":[],"value_type":"boolean","cardinality":"one","applicable_categories":[],
            "allowed_values":null,"scale":null,"parent":null,"status":"active"}),
    )
    .await;
    add_concept(
        &core,
        &app,
        community,
        "role",
        json!({"key":"role.test","name":"Test role","kind":"role","definition":"Role",
            "aliases":[],"value_type":"boolean","cardinality":"one","applicable_categories":["domain.test"],
            "allowed_values":null,"scale":null,"parent":null,"status":"active"}),
    )
    .await;
    add_concept(
        &core,
        &app,
        community,
        "dimension",
        json!({"key":"dimension.test.score","name":"Assessment","kind":"dimension","definition":"Assessment",
            "aliases":[],"value_type":"integer","cardinality":"one","applicable_categories":["role.test"],
            "allowed_values":null,"scale":{"min":0,"max":10,"rubric":"test-rubric","version":"1",
            "anchors":{"0":"none","10":"full"}},"parent":null,"status":"active"}),
    )
    .await;
    add_tool(&core, &app, community, "scoped-tool").await;

    let inapplicable = core
        .call_authenticated(
            &app,
            "toolkit.assert",
            assertion(
                community,
                "role-too-early",
                "scoped-tool",
                "role.test",
                json!(true),
                "proposed",
            ),
        )
        .await
        .unwrap_err();
    assert!(inapplicable.to_string().contains("concept_not_applicable"));
    call(
        &core,
        &app,
        "toolkit.assert",
        assertion(
            community,
            "domain",
            "scoped-tool",
            "domain.test",
            json!(true),
            "verified",
        ),
    )
    .await;
    call(
        &core,
        &app,
        "toolkit.assert",
        assertion(
            community,
            "role",
            "scoped-tool",
            "role.test",
            json!(true),
            "verified",
        ),
    )
    .await;

    let incomplete = core
        .call_authenticated(
            &app,
            "toolkit.assert",
            assertion(
                community,
                "score-bad",
                "scoped-tool",
                "dimension.test.score",
                json!(11),
                "verified",
            ),
        )
        .await
        .unwrap_err();
    assert!(incomplete.to_string().contains("value_out_of_range"));
    let incomplete = core
        .call_authenticated(
            &app,
            "toolkit.assert",
            assertion(
                community,
                "score-incomplete",
                "scoped-tool",
                "dimension.test.score",
                json!(8),
                "verified",
            ),
        )
        .await
        .unwrap_err();
    assert!(incomplete.to_string().contains("incomplete_assessment"));
    let mut assessment = assertion(
        community,
        "score",
        "scoped-tool",
        "dimension.test.score",
        json!(8),
        "verified",
    );
    let object = assessment.as_object_mut().unwrap();
    object.insert("rubric".into(), json!("test-rubric"));
    object.insert("rubric_version".into(), json!("1"));
    object.insert("rationale".into(), json!("Evidence maps to anchor eight."));
    object.insert("evaluator_type".into(), json!("human"));
    object.insert("evaluation_date".into(), json!("2026-07-26"));
    call(&core, &app, "toolkit.assert", assessment).await;

    let app_b = core.authenticate(&token_b).await.unwrap().unwrap();
    let other_app = call(
        &core,
        &app_b,
        "toolkit.schema.list",
        json!({"community_id":community}),
    )
    .await;
    assert!(other_app.as_array().unwrap().is_empty());
    let other_community = call(
        &core,
        &app,
        "toolkit.schema.list",
        json!({"community_id":"another"}),
    )
    .await;
    assert!(other_community.as_array().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_projection_commit_leaves_no_loro_or_acknowledged_state() {
    let temp = TempDir::new().unwrap();
    config::initialize_data_dir(temp.path()).unwrap();
    let db = config::database_path(temp.path());
    let (hash, token) = config::generate_token().unwrap();
    sqlite::register_application(&db, "org.test.failpoint", &hash, "APP").unwrap();
    let connection = Connection::open(&db).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER fail_toolkit_projection BEFORE INSERT ON toolkit_tools
             WHEN NEW.tool_key='fail-tool'
             BEGIN SELECT RAISE(ABORT, 'injected toolkit projection failure'); END;",
        )
        .unwrap();
    drop(connection);
    let (core, _) = core(&db, temp.path(), 101);
    let app = core.authenticate(&token).await.unwrap().unwrap();
    let error = core
        .call_authenticated(
            &app,
            "toolkit.tool.add",
            json!({
                "community_id":"failure", "idempotency_key":"fail-command",
                "key":"fail-tool", "name":"Fail Tool"
            }),
        )
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("injected toolkit projection failure")
    );
    let document = call(
        &core,
        &app,
        "document.get",
        json!({"community_id":"failure","document_id":"toolkit/tools/fail-tool"}),
    )
    .await;
    assert_eq!(document["update_count"], 0);
    assert_eq!(document["state"], json!({}));
    let reader = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    for table in [
        "operations",
        "durable_outbox",
        "toolkit_tools",
        "idempotency_keys",
    ] {
        let count: i64 = reader
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "{table}");
    }
}

async fn seed_fixture(core: &CommunityCore, app: &Principal, community: &str) {
    let taxonomy: Value =
        serde_json::from_str(include_str!("fixtures/toolkit/taxonomy-0.1.json")).unwrap();
    let corpus: Value =
        serde_json::from_str(include_str!("fixtures/toolkit/corpus-0.1.json")).unwrap();
    for (index, concept) in taxonomy["concepts"].as_array().unwrap().iter().enumerate() {
        add_concept(
            core,
            app,
            community,
            &format!("fixture-{index}"),
            concept.clone(),
        )
        .await;
    }
    for (index, tool) in corpus["tools"].as_array().unwrap().iter().enumerate() {
        let mut params = tool.as_object().unwrap().clone();
        params.insert("community_id".into(), json!(community));
        params.insert(
            "idempotency_key".into(),
            json!(format!("fixture-tool-{index}")),
        );
        call(core, app, "toolkit.tool.add", Value::Object(params)).await;
    }
    for (index, item) in corpus["assertions"].as_array().unwrap().iter().enumerate() {
        let mut params = item.as_object().unwrap().clone();
        params.insert("community_id".into(), json!(community));
        params.insert(
            "idempotency_key".into(),
            json!(format!("fixture-assert-{index}")),
        );
        params.insert("assertion_id".into(), json!(format!("fixture-{index:04}")));
        call(core, app, "toolkit.assert", Value::Object(params)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn shared_eighteen_query_benchmark_is_exact_and_export_is_deterministic() {
    let (_temp, core, app, _, _, _) = setup("org.test.benchmark").await;
    seed_fixture(&core, &app, "benchmark").await;
    let benchmark: Value =
        serde_json::from_str(include_str!("fixtures/toolkit/queries-0.1.json")).unwrap();
    for case in benchmark["queries"].as_array().unwrap() {
        let mut params = case["plan"].as_object().unwrap().clone();
        params.insert("community_id".into(), json!("benchmark"));
        let result = call(&core, &app, "toolkit.query", Value::Object(params)).await;
        let mut exact = result["results"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|result| result["match"] == "exact")
            .map(|result| result["tool"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        exact.sort();
        let mut expected = case["expected_exact"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool.as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        expected.sort();
        assert_eq!(exact, expected, "{}", case["id"]);
    }
    let first = call(
        &core,
        &app,
        "toolkit.export",
        json!({"community_id":"benchmark","limit":500}),
    )
    .await;
    let second = call(
        &core,
        &app,
        "toolkit.export",
        json!({"community_id":"benchmark","limit":500}),
    )
    .await;
    assert_eq!(first, second);
    assert_eq!(first["concepts"].as_array().unwrap().len(), 36);
    assert_eq!(first["tools"].as_array().unwrap().len(), 12);
    assert_eq!(first["assertions"].as_array().unwrap().len(), 87);
    assert_eq!(first["import_supported"], false);
}
