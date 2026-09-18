use std::sync::Arc;

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
};
use serde_json::json;
use tempfile::TempDir;

fn core(db: &std::path::Path, data: &std::path::Path, peer: u64) -> (CommunityCore, [u8; 32]) {
    let (signing_key, master_key) = config::load_keys(data).unwrap();
    let core = CommunityCore::new(
        Arc::new(StoreHandle::start(db).unwrap()),
        Arc::new(LoroDocumentEngine),
        Arc::new(P2pandaSecureLog::new(signing_key, master_key)),
        Arc::new(Blake3ContentHasher),
        peer,
    );
    (core, master_key)
}

#[tokio::test]
async fn durable_write_is_idempotent_and_reaches_opaque_outbox() {
    let temp = TempDir::new().unwrap();
    config::initialize_data_dir(temp.path()).unwrap();
    let db = config::database_path(temp.path());
    sqlite::initialize(&db).unwrap();

    let (app_hash, app_token) = config::generate_token().unwrap();
    sqlite::register_application(&db, "org.test.wiki", &app_hash, "APP").unwrap();
    let (transport_hash, transport_token) = config::generate_token().unwrap();
    sqlite::register_application(&db, "reticulum", &transport_hash, "TRANSPORT").unwrap();
    let (core, master_key) = core(&db, temp.path(), 42);
    let app = core.authenticate(&app_token).await.unwrap().unwrap();
    let transport = core.authenticate(&transport_token).await.unwrap().unwrap();

    let params = json!({
        "community_id": "test-community",
        "document_id": "welcome",
        "idempotency_key": "command-1",
        "schema_version": 1,
        "mutations": [
            {"op": "text_insert", "container": "body", "index": 0, "text": "hello"},
            {"op": "map_set", "container": "meta", "key": "published", "value": true}
        ]
    });
    let first = core
        .call_authenticated(&app, "document.mutate", params.clone())
        .await
        .unwrap();
    let retry = core
        .call_authenticated(&app, "document.mutate", params)
        .await
        .unwrap();
    assert_eq!(first, retry);
    assert_eq!(first["durable"], true);
    let mut different = json!({
        "community_id": "test-community",
        "document_id": "welcome",
        "idempotency_key": "command-1",
        "schema_version": 1,
        "mutations": [
            {"op": "text_insert", "container": "body", "index": 0, "text": "different"}
        ]
    });
    let conflict = core
        .call_authenticated(&app, "document.mutate", different.take())
        .await
        .unwrap_err();
    assert!(
        conflict
            .to_string()
            .contains("idempotency key was already used")
    );

    let document = core
        .call_authenticated(
            &app,
            "document.get",
            json!({"community_id": "test-community", "document_id": "welcome"}),
        )
        .await
        .unwrap();
    assert_eq!(document["state"]["body"], "hello");
    assert_eq!(document["state"]["meta"]["published"], true);
    assert_eq!(document["update_count"], 1);

    let claimed = core
        .call_authenticated(
            &transport,
            "outbox.claim",
            json!({"transport": "reticulum", "limit": 8, "max_bytes": 262_144}),
        )
        .await
        .unwrap();
    let item = &claimed.as_array().unwrap()[0];
    let header = base64::engine::general_purpose::STANDARD
        .decode(item["header_base64"].as_str().unwrap())
        .unwrap();
    let body = base64::engine::general_purpose::STANDARD
        .decode(item["body_base64"].as_str().unwrap())
        .unwrap();
    let (decoded_header, payload) = decode_update(&header, &body, &master_key).unwrap();
    assert_eq!(decoded_header.extensions.app_id, "org.test.wiki");
    assert_eq!(decoded_header.extensions.document_id, "welcome");
    assert!(!payload.loro_update.is_empty());

    let ack = json!({
        "transport":"reticulum",
        "operation_hash":item["operation_hash"],
        "lease_attempt":item["lease_attempt"],
        "status":"STORED"
    });
    core.call_authenticated(&transport, "outbox.ack", ack.clone())
        .await
        .unwrap();
    assert!(
        core.call_authenticated(&transport, "outbox.ack", ack)
            .await
            .unwrap_err()
            .to_string()
            .contains("stale")
    );
}

#[tokio::test]
async fn facts_are_loro_authoritative_and_sql_queryable() {
    let temp = TempDir::new().unwrap();
    config::initialize_data_dir(temp.path()).unwrap();
    let db = config::database_path(temp.path());
    let (hash, token) = config::generate_token().unwrap();
    sqlite::register_application(&db, "org.test.knowledge", &hash, "APP").unwrap();
    let (admin_hash, admin_token) = config::generate_token().unwrap();
    sqlite::register_application(&db, "local-admin", &admin_hash, "ADMIN").unwrap();
    let (core, _) = core(&db, temp.path(), 73);
    let app = core.authenticate(&token).await.unwrap().unwrap();
    let admin = core.authenticate(&admin_token).await.unwrap().unwrap();
    core.call_authenticated(
        &admin,
        "schema.register",
        json!({
            "app_id":"org.test.knowledge", "schema_id":"org.test.knowledge/located_at",
            "schema_version":1, "predicate":"org.test.knowledge/located_at@1",
            "object_schema":{"type":"object","properties":{"shelf":{"type":"string","maxLength":32}},"required":["shelf"],"additionalProperties":false},
            "multiple_active_claims":false, "provenance_policy":"SOURCE"
        }),
    ).await.unwrap();

    let asserted = core
        .call_authenticated(
            &app,
            "fact.assert",
            json!({
                "community_id": "library",
                "claim_id": "claim-1",
                "subject": "book:123",
                "predicate": "org.test.knowledge/located_at@1",
                "object": {"shelf": "A4"},
                "source": "inventory-2026",
                "confidence": 0.95,
                "schema_id":"org.test.knowledge/located_at",
                "schema_version":1,
                "idempotency_key": "fact-command-1"
            }),
        )
        .await
        .unwrap();
    assert_eq!(asserted["durable"], true);
    assert_eq!(asserted["state"]["claim"]["subject"], "book:123");

    let facts = core
        .call_authenticated(
            &app,
            "fact.query",
            json!({"community_id": "library", "subject": "book:123"}),
        )
        .await
        .unwrap();
    assert_eq!(facts[0]["claim_id"], "claim-1");
    assert_eq!(facts[0]["object"]["shelf"], "A4");
}

#[tokio::test]
async fn code_owned_namespaces_cannot_be_registered_as_principals() {
    let temp = TempDir::new().unwrap();
    config::initialize_data_dir(temp.path()).unwrap();
    let db = config::database_path(temp.path());
    let (hash, _) = config::generate_token().unwrap();
    assert!(
        sqlite::register_application(&db, "community.planning", &hash, "APP")
            .unwrap_err()
            .to_string()
            .contains("reserved")
    );
    let (core, _) = core(&db, temp.path(), 9);
    let forged_principal = community_stack::domain::Principal {
        id: "community.planning".into(),
        role: community_stack::domain::PrincipalRole::App,
    };
    assert!(
        core.call_authenticated(
            &forged_principal,
            "document.get",
            json!({"community_id":"team","document_id":"planning/projects/secret"}),
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("reserved")
    );
}

#[tokio::test]
async fn principal_role_rotation_revokes_every_old_token() {
    let temp = TempDir::new().unwrap();
    config::initialize_data_dir(temp.path()).unwrap();
    let db = config::database_path(temp.path());
    let (app_hash, app_token) = config::generate_token().unwrap();
    sqlite::register_application(&db, "rotating-principal", &app_hash, "APP").unwrap();
    let (admin_hash, admin_token) = config::generate_token().unwrap();
    sqlite::register_application(&db, "rotating-principal", &admin_hash, "ADMIN").unwrap();
    let (transport_hash, transport_token) = config::generate_token().unwrap();
    sqlite::register_application(&db, "rotating-principal", &transport_hash, "TRANSPORT").unwrap();
    let (core, _) = core(&db, temp.path(), 8);
    assert!(core.authenticate(&app_token).await.unwrap().is_none());
    assert!(core.authenticate(&admin_token).await.unwrap().is_none());
    let principal = core.authenticate(&transport_token).await.unwrap().unwrap();
    assert_eq!(
        principal.role,
        community_stack::domain::PrincipalRole::Transport
    );
}

#[tokio::test]
async fn capabilities_are_role_scoped() {
    let temp = TempDir::new().unwrap();
    config::initialize_data_dir(temp.path()).unwrap();
    let db = config::database_path(temp.path());
    let (hash, token) = config::generate_token().unwrap();
    sqlite::register_application(&db, "org.test.app", &hash, "APP").unwrap();
    let (core, _) = core(&db, temp.path(), 7);
    let app = core.authenticate(&token).await.unwrap().unwrap();

    let error = core
        .call_authenticated(
            &app,
            "outbox.claim",
            json!({"transport": "reticulum", "limit": 1, "max_bytes": 1024}),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("not allowed"));
}
