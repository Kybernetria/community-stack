#![allow(clippy::too_many_lines)]

use std::{
    path::Path,
    sync::{Arc, Barrier},
};

use community_stack::{
    adapters::{
        hashing::Blake3ContentHasher,
        loro::LoroDocumentEngine,
        p2panda::P2pandaSecureLog,
        sqlite::{self, StoreHandle},
    },
    application::CommunityCore,
    config,
    domain::Principal,
};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use tempfile::TempDir;

const APP_ID: &str = "org.example.library";
const SCHEMA_ID: &str = "org.example.library/located_at";
const PREDICATE: &str = "org.example.library/located_at@1";

fn make_core(db: &Path, data: &Path, peer: u64) -> CommunityCore {
    let (signing_key, master_key) = config::load_keys(data).unwrap();
    CommunityCore::new(
        Arc::new(StoreHandle::start(db).unwrap()),
        Arc::new(LoroDocumentEngine),
        Arc::new(P2pandaSecureLog::new(signing_key, master_key)),
        Arc::new(Blake3ContentHasher),
        peer,
    )
}

async fn setup() -> (TempDir, CommunityCore, Principal, Principal, Principal) {
    let temp = TempDir::new().unwrap();
    config::initialize_data_dir(temp.path()).unwrap();
    let db = config::database_path(temp.path());
    let (app_hash, app_token) = config::generate_token().unwrap();
    sqlite::register_application(&db, APP_ID, &app_hash, "APP").unwrap();
    let (admin_hash, admin_token) = config::generate_token().unwrap();
    sqlite::register_application(&db, "device-admin", &admin_hash, "ADMIN").unwrap();
    let (transport_hash, transport_token) = config::generate_token().unwrap();
    sqlite::register_application(&db, "mesh", &transport_hash, "TRANSPORT").unwrap();
    let core = make_core(&db, temp.path(), 818);
    let app = core.authenticate(&app_token).await.unwrap().unwrap();
    let admin = core.authenticate(&admin_token).await.unwrap().unwrap();
    let transport = core.authenticate(&transport_token).await.unwrap().unwrap();
    (temp, core, app, admin, transport)
}

async fn register_schema(core: &CommunityCore, admin: &Principal) {
    core.call_authenticated(
        admin,
        "schema.register",
        json!({
            "app_id":APP_ID,
            "schema_id":SCHEMA_ID,
            "schema_version":1,
            "predicate":PREDICATE,
            "object_schema":{
                "type":"object",
                "properties":{
                    "shelf":{"type":"string","maxLength":32},
                    "aisle":{"type":"integer","minimum":0,"maximum":100}
                },
                "required":["shelf"],
                "additionalProperties":false
            },
            "multiple_active_claims":false,
            "provenance_policy":"SOURCE_AND_DOCUMENT",
            "lifecycle_policy":{
                "allow_supersession":true,
                "allow_retraction":true,
                "allow_dispute":true,
                "allow_expiry":false
            }
        }),
    )
    .await
    .unwrap();
}

fn assertion(key: &str, shelf: &str) -> Value {
    json!({
        "community_id":"library",
        "claim_id":"claim-1",
        "subject":"book:123",
        "predicate":PREDICATE,
        "object":{"shelf":shelf,"aisle":4},
        "source":"inventory-import",
        "source_document_id":"inventory/2026-07",
        "confidence":0.95,
        "schema_id":SCHEMA_ID,
        "schema_version":1,
        "idempotency_key":key
    })
}

fn count(connection: &Connection, table: &str) -> i64 {
    connection
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn governed_fact_revision_history_inspection_and_restart_are_coherent() {
    let (temp, core, app, admin, _) = setup().await;
    let db = config::database_path(temp.path());
    register_schema(&core, &admin).await;
    let schemas = core
        .call_authenticated(&admin, "schema.list", json!({"app_id":APP_ID,"limit":10}))
        .await
        .unwrap();
    assert_eq!(schemas.as_array().unwrap().len(), 1);

    let first_params = assertion("fact-1", "A4");
    let first = core
        .call_authenticated(&app, "fact.assert", first_params.clone())
        .await
        .unwrap();
    let retry = core
        .call_authenticated(&app, "fact.assert", first_params)
        .await
        .unwrap();
    assert_eq!(first, retry);
    let reader = Connection::open(&db).unwrap();
    assert_eq!(count(&reader, "fact_claim_revisions"), 1);
    assert_eq!(count(&reader, "fact_claims"), 1);
    assert_eq!(count(&reader, "operations"), 1);
    assert_eq!(count(&reader, "document_updates"), 1);
    assert_eq!(count(&reader, "durable_outbox"), 1);
    assert_eq!(count(&reader, "idempotency_keys"), 1);
    drop(reader);

    let second = core
        .call_authenticated(&app, "fact.assert", assertion("fact-2", "B2"))
        .await
        .unwrap();
    assert_ne!(first["operation_hash"], second["operation_hash"]);
    let reader = Connection::open(&db).unwrap();
    assert_eq!(count(&reader, "fact_claim_revisions"), 2);
    let first_status: String = reader
        .query_row(
            "SELECT lifecycle_status FROM fact_claim_revisions WHERE operation_hash=?1",
            [hex::decode(first["operation_hash"].as_str().unwrap()).unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(first_status, "SUPERSEDED");
    drop(reader);

    let inspection = core
        .call_authenticated(
            &app,
            "fact.inspect",
            json!({"community_id":"library","claim_id":"claim-1"}),
        )
        .await
        .unwrap();
    assert_eq!(
        inspection["current_revision_operation_hash"],
        second["operation_hash"]
    );
    assert_eq!(inspection["loro_document_id"], "facts/claim-1");
    assert_eq!(inspection["revision_count"], 2);
    assert_eq!(inspection["projection_consistent"], true);
    assert_eq!(inspection["schema_id"], SCHEMA_ID);
    assert_eq!(
        inspection["supersedes_operation_hash"],
        first["operation_hash"]
    );

    let page_one = core
        .call_authenticated(
            &app,
            "fact.history",
            json!({"community_id":"library","claim_id":"claim-1","limit":1}),
        )
        .await
        .unwrap();
    let cursor = page_one["next_cursor"].as_str().unwrap();
    assert!(
        core.call_authenticated(
            &app,
            "fact.history",
            json!({"community_id":"library","claim_id":"claim-1","limit":101}),
        )
        .await
        .is_err()
    );
    let page_two = core
        .call_authenticated(
            &app,
            "fact.history",
            json!({"community_id":"library","claim_id":"claim-1","limit":1,"cursor":cursor}),
        )
        .await
        .unwrap();
    assert_eq!(
        page_one["revisions"][0]["operation_hash"],
        second["operation_hash"]
    );
    assert_eq!(
        page_two["revisions"][0]["operation_hash"],
        first["operation_hash"]
    );

    let mut retraction = assertion("fact-3", "B2");
    retraction["lifecycle_status"] = json!("RETRACTED");
    let retracted = core
        .call_authenticated(&app, "fact.assert", retraction)
        .await
        .unwrap();
    let history = core
        .call_authenticated(
            &app,
            "fact.history",
            json!({"community_id":"library","claim_id":"claim-1","limit":10}),
        )
        .await
        .unwrap();
    assert_eq!(history["revisions"].as_array().unwrap().len(), 3);
    assert_eq!(
        history["revisions"][0]["operation_hash"],
        retracted["operation_hash"]
    );
    assert_eq!(history["revisions"][0]["lifecycle_status"], "RETRACTED");

    let restarted = make_core(&db, temp.path(), 819);
    let after_restart = restarted
        .call_authenticated(
            &app,
            "fact.inspect",
            json!({"community_id":"library","claim_id":"claim-1"}),
        )
        .await
        .unwrap();
    assert_eq!(
        after_restart["current_revision_operation_hash"],
        retracted["operation_hash"]
    );
    assert_eq!(after_restart["revision_count"], 3);

    let doctor = restarted
        .call_authenticated(&admin, "system.doctor", json!({}))
        .await
        .unwrap();
    assert!(
        doctor["checks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|check| check["status"] == "OK")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn single_active_fact_policy_is_transactional_and_terminal() {
    let (temp, core, app, admin, _) = setup().await;
    let db = config::database_path(temp.path());
    register_schema(&core, &admin).await;
    let second_core = make_core(&db, temp.path(), 820);

    let mut first = assertion("concurrent-1", "A1");
    first["claim_id"] = json!("concurrent-1");
    let mut second = assertion("concurrent-2", "A2");
    second["claim_id"] = json!("concurrent-2");
    let (left, right) = tokio::join!(
        core.call_authenticated(&app, "fact.assert", first),
        second_core.call_authenticated(&app, "fact.assert", second)
    );
    assert_ne!(left.is_ok(), right.is_ok());
    assert!(
        left.as_ref()
            .err()
            .or_else(|| right.as_ref().err())
            .unwrap()
            .to_string()
            .contains("only one active claim")
    );

    let winning_claim = if left.is_ok() {
        "concurrent-1"
    } else {
        "concurrent-2"
    };
    core.call_authenticated(
        &admin,
        "schema.register",
        json!({
            "app_id":APP_ID,
            "schema_id":"org.example.library/alternate",
            "schema_version":1,
            "predicate":"org.example.library/alternate@1",
            "object_schema":{"type":"object","properties":{"shelf":{"type":"string","maxLength":32},"aisle":{"type":"integer","minimum":0,"maximum":100}},"required":["shelf"],"additionalProperties":false},
            "multiple_active_claims":true,
            "provenance_policy":"SOURCE_AND_DOCUMENT",
            "lifecycle_policy":{"allow_supersession":true,"allow_retraction":true,"allow_dispute":true,"allow_expiry":true}
        }),
    )
    .await
    .unwrap();
    let mut switched = assertion("schema-switch", "A1");
    switched["claim_id"] = json!(winning_claim);
    switched["schema_id"] = json!("org.example.library/alternate");
    switched["predicate"] = json!("org.example.library/alternate@1");
    switched["lifecycle_status"] = json!("EXPIRED");
    assert!(
        core.call_authenticated(&app, "fact.assert", switched)
            .await
            .unwrap_err()
            .to_string()
            .contains("immutable")
    );

    let mut retraction = assertion("terminal-retract", "A1");
    retraction["claim_id"] = json!(winning_claim);
    retraction["lifecycle_status"] = json!("RETRACTED");
    call_fact(&core, &app, retraction).await;
    let mut resurrection = assertion("terminal-resurrect", "A3");
    resurrection["claim_id"] = json!(winning_claim);
    assert!(
        core.call_authenticated(&app, "fact.assert", resurrection)
            .await
            .unwrap_err()
            .to_string()
            .contains("terminal")
    );

    let mut superseded = assertion("invalid-lifecycle", "A4");
    superseded["claim_id"] = json!("another-claim");
    superseded["lifecycle_status"] = json!("SUPERSEDED");
    assert!(
        core.call_authenticated(&app, "fact.assert", superseded)
            .await
            .is_err()
    );
}

async fn call_fact(core: &CommunityCore, app: &Principal, params: Value) -> Value {
    core.call_authenticated(app, "fact.assert", params)
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn validation_capabilities_and_projection_verification_fail_closed() {
    let (temp, core, app, admin, transport) = setup().await;
    let db = config::database_path(temp.path());
    register_schema(&core, &admin).await;

    for (principal, method, params) in [
        (&app, "schema.list", json!({"app_id":APP_ID})),
        (&admin, "fact.query", json!({"community_id":"library"})),
        (&transport, "system.doctor", json!({})),
    ] {
        assert!(
            core.call_authenticated(principal, method, params)
                .await
                .unwrap_err()
                .to_string()
                .contains("not allowed")
        );
    }

    let reader = Connection::open(&db).unwrap();
    let before = (
        count(&reader, "operations"),
        count(&reader, "document_updates"),
        count(&reader, "fact_claim_revisions"),
        count(&reader, "durable_outbox"),
    );
    drop(reader);
    let mut malformed = assertion("bad-shape", "A4");
    malformed["object"] = json!({"unknown":"value"});
    assert!(
        core.call_authenticated(&app, "fact.assert", malformed)
            .await
            .unwrap_err()
            .to_string()
            .contains("not allowed")
    );
    let mut unregistered = assertion("bad-schema", "A4");
    unregistered["schema_id"] = json!("org.example.library/missing");
    unregistered["predicate"] = json!("org.example.library/missing@1");
    assert!(
        core.call_authenticated(&app, "fact.assert", unregistered)
            .await
            .is_err()
    );
    let reader = Connection::open(&db).unwrap();
    assert_eq!(
        before,
        (
            count(&reader, "operations"),
            count(&reader, "document_updates"),
            count(&reader, "fact_claim_revisions"),
            count(&reader, "durable_outbox"),
        )
    );
    drop(reader);

    core.call_authenticated(&app, "fact.assert", assertion("good", "A4"))
        .await
        .unwrap();
    let bytes_before: (Vec<u8>, Vec<u8>) = Connection::open(&db)
        .unwrap()
        .query_row(
            "SELECT canonical_header,body_ciphertext FROM operations LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let clean = core
        .call_authenticated(&admin, "projection.check", json!({}))
        .await
        .unwrap();
    assert_eq!(clean["consistent"], true);
    Connection::open(&db)
        .unwrap()
        .execute(
            "UPDATE fact_claims SET subject='corrupted-projection' WHERE claim_id='claim-1'",
            [],
        )
        .unwrap();
    let corrupt = core
        .call_authenticated(&admin, "projection.check", json!({}))
        .await
        .unwrap();
    assert_eq!(corrupt["consistent"], false);
    assert_eq!(corrupt["rebuild_enabled"], false);
    let disabled = core
        .call_authenticated(&admin, "projection.rebuild", json!({}))
        .await
        .unwrap();
    assert_eq!(disabled["enabled"], false);
    assert_eq!(disabled["applied"], false);
    let bytes_after: (Vec<u8>, Vec<u8>) = Connection::open(&db)
        .unwrap()
        .query_row(
            "SELECT canonical_header,body_ciphertext FROM operations LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(bytes_before, bytes_after);
}

#[test]
fn migration_artifact_digests_are_pinned() {
    for (bytes, expected) in [
        (
            include_bytes!("../migrations/0001_core.sql").as_slice(),
            "9bbdbdb32bfa205624db23724fb13b10c1ba0c8b19b549c66153b8b683c2b2c0",
        ),
        (
            include_bytes!("../migrations/0002_toolkit.sql").as_slice(),
            "a07e36958351e4633e8fe978cd213a11d97417512c4b1e139d1f415ad0fb0bb5",
        ),
        (
            include_bytes!("../migrations/0003_fact_governance.sql").as_slice(),
            "c6a1ff0c8368f95a301a6e2ca1048d0cdedac55c462463b0331521df3b189cd6",
        ),
        (
            include_bytes!("../migrations/0004_planning_profile.sql").as_slice(),
            "6d7d7893d1836c94fbfdd3720d88bfb0fc7cb987131963e304248feabfa5a3f1",
        ),
        (
            include_bytes!("../migrations/0005_authorization_and_invariants.sql").as_slice(),
            "241a9e34078d5a60998c15dc99cce9f35048c059a5a6e44714ca898415126963",
        ),
    ] {
        assert_eq!(blake3::hash(bytes).to_hex().as_str(), expected);
    }
}

#[test]
fn concurrent_initializers_apply_each_migration_once() {
    let temp = TempDir::new().unwrap();
    let db = Arc::new(temp.path().join("concurrent.sqlite3"));
    let barrier = Arc::new(Barrier::new(4));
    let handles = (0..4)
        .map(|_| {
            let db = Arc::clone(&db);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                sqlite::initialize(&db)
            })
        })
        .collect::<Vec<_>>();
    for handle in handles {
        handle.join().unwrap().unwrap();
    }
    let connection = Connection::open(db.as_ref()).unwrap();
    assert_eq!(count(&connection, "schema_migrations"), 5);
    let checksums: i64 = connection
        .query_row(
            "SELECT count(*) FROM schema_migrations WHERE length(checksum)=64",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(checksums, 5);
}

#[test]
fn newer_or_checksum_mismatched_migrations_fail_closed() {
    let temp = TempDir::new().unwrap();
    let db = temp.path().join("future.sqlite3");
    sqlite::initialize(&db).unwrap();
    let connection = Connection::open(&db).unwrap();
    connection
        .execute(
            "INSERT INTO schema_migrations(version,applied_at_ms,checksum) VALUES(99,1,'future')",
            [],
        )
        .unwrap();
    drop(connection);
    assert!(
        sqlite::initialize(&db)
            .unwrap_err()
            .to_string()
            .contains("newer unsupported")
    );

    let db = temp.path().join("checksum.sqlite3");
    sqlite::initialize(&db).unwrap();
    Connection::open(&db)
        .unwrap()
        .execute(
            "UPDATE schema_migrations SET checksum='tampered' WHERE version=3",
            [],
        )
        .unwrap();
    assert!(
        sqlite::initialize(&db)
            .unwrap_err()
            .to_string()
            .contains("checksum mismatch")
    );
}

#[test]
fn ambiguous_legacy_principals_fail_with_recovery_guidance() {
    let temp = TempDir::new().unwrap();
    let db = temp.path().join("ambiguous.sqlite3");
    let connection = Connection::open(&db).unwrap();
    for (version, sql) in [
        (1_i64, include_str!("../migrations/0001_core.sql")),
        (2_i64, include_str!("../migrations/0002_toolkit.sql")),
        (
            3_i64,
            include_str!("../migrations/0003_fact_governance.sql"),
        ),
        (
            4_i64,
            include_str!("../migrations/0004_planning_profile.sql"),
        ),
    ] {
        connection.execute_batch(sql).unwrap();
        connection
            .execute(
                "INSERT INTO schema_migrations(version,applied_at_ms) VALUES(?1,?1)",
                [version],
            )
            .unwrap();
    }
    connection
        .execute(
            "INSERT INTO applications(app_id,token_hash,role,enabled,created_at_ms) VALUES('duplicate',?1,'APP',1,1)",
            [[1_u8; 32].as_slice()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO administrators(principal_id,token_hash,enabled,created_at_ms) VALUES('duplicate',?1,1,1)",
            [[2_u8; 32].as_slice()],
        )
        .unwrap();
    drop(connection);
    let error = sqlite::initialize(&db).unwrap_err().to_string();
    assert!(error.contains("ambiguous"));
    assert!(error.contains("rotate"));
}

#[test]
fn version_one_database_upgrades_once_without_data_loss() {
    let temp = TempDir::new().unwrap();
    let db = temp.path().join("v1.sqlite3");
    let connection = Connection::open(&db).unwrap();
    connection
        .execute_batch(include_str!("../migrations/0001_core.sql"))
        .unwrap();
    connection
        .execute(
            "INSERT INTO schema_migrations(version,applied_at_ms) VALUES(1,1)",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO applications(app_id,token_hash,role,enabled,created_at_ms) VALUES(?1,?2,'APP',1,1)",
            params![APP_ID, [7_u8;32].as_slice()],
        )
        .unwrap();
    let operation_hash = [1_u8; 32];
    connection.execute(
        "INSERT INTO operations(operation_hash,canonical_header,body_ciphertext,author_key,log_id,generation,sequence,backlink,app_id,community_id,document_id,record_kind,verification_status,apply_status,received_at_ms) VALUES(?1,?2,?3,?4,?5,0,0,NULL,?6,'library','facts/legacy',1,'VERIFIED','APPLIED',10)",
        params![operation_hash.as_slice(), b"canonical-v1", b"cipher-v1", [2_u8;32].as_slice(), [3_u8;32].as_slice(), APP_ID],
    ).unwrap();
    connection.execute(
        "INSERT INTO document_updates(operation_hash,app_id,community_id,document_id,codec,key_epoch,update_bytes,applied,pending_deps,created_at_ms) VALUES(?1,?2,'library','facts/legacy',1,1,?3,1,NULL,10)",
        params![operation_hash.as_slice(), APP_ID, b"loro-v1"],
    ).unwrap();
    connection.execute(
        "INSERT INTO fact_claims(app_id,community_id,claim_id,subject,predicate,object_json,source,confidence,source_operation_hash,retracted,updated_at_ms) VALUES(?1,'library','legacy','book:old','located_at',?2,'old-import',1.0,?3,0,10)",
        params![APP_ID, br#"{"shelf":"old"}"#, operation_hash.as_slice()],
    ).unwrap();
    drop(connection);

    sqlite::initialize(&db).unwrap();
    sqlite::initialize(&db).unwrap();
    let upgraded = Connection::open(&db).unwrap();
    assert_eq!(count(&upgraded, "operations"), 1);
    assert_eq!(count(&upgraded, "document_updates"), 1);
    assert_eq!(count(&upgraded, "fact_claims"), 1);
    assert_eq!(count(&upgraded, "fact_claim_revisions"), 1);
    assert_eq!(count(&upgraded, "schema_migrations"), 5);
    let preserved: (Vec<u8>, Vec<u8>, Vec<u8>) = upgraded
        .query_row(
            "SELECT o.canonical_header,o.body_ciphertext,d.update_bytes FROM operations o JOIN document_updates d USING(operation_hash)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        preserved,
        (
            b"canonical-v1".to_vec(),
            b"cipher-v1".to_vec(),
            b"loro-v1".to_vec()
        )
    );
    let legacy: (String, u32) = upgraded
        .query_row(
            "SELECT schema_id,schema_version FROM fact_claim_revisions",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(legacy, ("legacy/untyped".into(), 0));
    let legacy_source_document: Option<String> = upgraded
        .query_row(
            "SELECT source_document_id FROM fact_claim_revisions",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(legacy_source_document.is_none());
}
