use community_stack::{
    adapters::{
        hashing::Blake3ContentHasher,
        loro::LoroDocumentEngine,
        p2panda::P2pandaSecureLog,
        sqlite::{self, StoreHandle},
    },
    application::CommunityCore,
    config,
    domain::{DocumentKey, ForgeDocumentUpdate, Mutation, PrimitiveValue, Principal},
    ports::{DocumentEngine, DocumentRepository, LocalCommit, SecureLog},
    recovery,
};
use serde_json::{Value, json};
use std::{fs, os::unix::fs::PermissionsExt, path::Path, sync::Arc};
use tempfile::TempDir;

fn make_core(data: &Path, peer: u64) -> CommunityCore {
    let (signing, master) = config::load_keys(data).unwrap();
    CommunityCore::new(
        Arc::new(StoreHandle::start(&config::database_path(data)).unwrap()),
        Arc::new(LoroDocumentEngine),
        Arc::new(P2pandaSecureLog::new(signing, master)),
        Arc::new(Blake3ContentHasher),
        peer,
    )
}
struct Fixture {
    temp: TempDir,
    core: CommunityCore,
    app: Principal,
    other: Principal,
    admin: Principal,
    token: String,
}
impl Fixture {
    async fn new() -> Self {
        let temp = TempDir::new().unwrap();
        config::initialize_data_dir(temp.path()).unwrap();
        let db = config::database_path(temp.path());
        let mut tokens = Vec::new();
        for (id, role) in [
            ("editor", "APP"),
            ("other-editor", "APP"),
            ("admin", "ADMIN"),
        ] {
            let (hash, token) = config::generate_token().unwrap();
            sqlite::register_application(&db, id, &hash, role).unwrap();
            tokens.push(token);
        }
        let core = make_core(temp.path(), 42);
        let app = core.authenticate(&tokens[0]).await.unwrap().unwrap();
        let other = core.authenticate(&tokens[1]).await.unwrap().unwrap();
        let admin = core.authenticate(&tokens[2]).await.unwrap().unwrap();
        Self {
            temp,
            core,
            app,
            other,
            admin,
            token: tokens.remove(0),
        }
    }
    async fn call(&self, method: &str, params: Value) -> Value {
        self.core
            .call_authenticated(&self.app, method, params)
            .await
            .unwrap()
    }
}
fn edit(document: &str, command: &str, revision: u64) -> Value {
    json!({"community_id":"garden","document_id":document,"idempotency_key":command,"expected_revision":revision,
        "mutations":[{"op":"text_insert","container":"body","index":0,"text":"🌱é"}]})
}
#[tokio::test]
async fn stale_edits_fail_but_committed_retries_return_the_original_ack() {
    let f = Fixture::new().await;
    let first = f.call("document.mutate", edit("note", "first", 0)).await;
    assert_eq!(first["revision"], 1);
    f.call("document.mutate", edit("note", "second", 1)).await;
    assert_eq!(
        first,
        f.call("document.mutate", edit("note", "first", 0)).await
    );
    let error = f
        .core
        .call_authenticated(&f.app, "document.mutate", edit("note", "stale", 1))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("document revision conflict"));
    let got = f
        .call(
            "document.get",
            json!({"community_id":"garden","document_id":"note"}),
        )
        .await;
    assert_eq!(got["revision"], 2);
    assert_eq!(got["state"]["body"], "🌱é🌱é");
    let changes = f
        .call("document.changes", json!({"community_id":"garden"}))
        .await;
    assert_eq!(changes["changes"].as_array().unwrap().len(), 2);
    let mut typo = edit("typo", "typo", 0);
    typo["expected_revison"] = json!(0);
    assert!(
        f.core
            .call_authenticated(&f.app, "document.mutate", typo)
            .await
            .is_err()
    );
}
#[tokio::test]
async fn discovery_and_feeds_are_literal_scoped_and_paginated() {
    let f = Fixture::new().await;
    for (i, name) in ["notes/a", "notes/b", "notes_%/literal", "table/a"]
        .iter()
        .enumerate()
    {
        f.call("document.mutate", edit(name, &format!("create-{i}"), 0))
            .await;
    }
    let page = f
        .call(
            "document.list",
            json!({"community_id":"garden","prefix":"notes/","limit":1}),
        )
        .await;
    assert_eq!(page["documents"][0]["document_id"], "notes/a");
    let page=f.call("document.list",json!({"community_id":"garden","prefix":"notes/","limit":1,"after":page["next_cursor"]})).await;
    assert_eq!(page["documents"][0]["document_id"], "notes/b");
    assert!(page["next_cursor"].is_null());
    let page = f
        .call(
            "document.list",
            json!({"community_id":"garden","prefix":"notes_%"}),
        )
        .await;
    assert_eq!(page["documents"].as_array().unwrap().len(), 1);
    for (method, field) in [
        ("document.list", "documents"),
        ("document.changes", "changes"),
    ] {
        let hidden = f
            .core
            .call_authenticated(&f.other, method, json!({"community_id":"garden"}))
            .await
            .unwrap();
        assert!(hidden[field].as_array().unwrap().is_empty());
        let hidden = f.call(method, json!({"community_id":"elsewhere"})).await;
        assert!(hidden[field].as_array().unwrap().is_empty());
        assert!(
            f.core
                .call_authenticated(&f.app, method, json!({"community_id":"garden","limit":0}))
                .await
                .is_err()
        );
    }
    let mut cursor = json!(0);
    let mut count = 0;
    loop {
        let page = f
            .call(
                "document.changes",
                json!({"community_id":"garden","limit":1,"after":cursor}),
            )
            .await;
        count += page["changes"].as_array().unwrap().len();
        cursor = page["next_cursor"].clone();
        if page["has_more"] == false {
            break;
        }
    }
    assert_eq!(count, 4);
    let empty = f
        .call(
            "document.changes",
            json!({"community_id":"garden","after":cursor}),
        )
        .await;
    assert_eq!(empty["next_cursor"], cursor);
    assert!(empty["changes"].as_array().unwrap().is_empty());
}
#[tokio::test]
async fn failed_batch_publishes_neither_state_nor_change() {
    let f = Fixture::new().await;
    let mut request = edit("bad", "bad", 0);
    request["mutations"]
        .as_array_mut()
        .unwrap()
        .push(json!({"op":"text_delete","container":"body","index":999,"length":1}));
    assert!(
        f.core
            .call_authenticated(&f.app, "document.mutate", request)
            .await
            .is_err()
    );
    let changes = f
        .call("document.changes", json!({"community_id":"garden"}))
        .await;
    assert!(changes["changes"].as_array().unwrap().is_empty());
    let doc = f
        .call(
            "document.get",
            json!({"community_id":"garden","document_id":"bad"}),
        )
        .await;
    assert_eq!(doc["revision"], 0);
}
#[tokio::test]
async fn stale_staged_state_is_rejected_even_with_the_latest_log_head() {
    let f = Fixture::new().await;
    f.call("document.mutate", edit("note", "first", 0)).await;
    let repo = StoreHandle::start(&config::database_path(f.temp.path())).unwrap();
    let (signing, master) = config::load_keys(f.temp.path()).unwrap();
    let secure = P2pandaSecureLog::new(signing, master);
    let key = DocumentKey {
        app_id: f.app.id.clone(),
        community_id: "garden".into(),
        document_id: "note".into(),
    };
    let change = LoroDocumentEngine
        .stage_mutations(
            &[],
            99,
            &[Mutation::MapSet {
                container: "meta".into(),
                key: "stale".into(),
                value: PrimitiveValue::Bool(true),
            }],
        )
        .unwrap();
    let head = repo
        .log_head(secure.author_key(), secure.document_log_id(&key))
        .await
        .unwrap()
        .unwrap();
    let record = secure
        .forge_document_update(ForgeDocumentUpdate {
            key: &key,
            sequence: head.sequence + 1,
            backlink: Some(head.operation_hash),
            schema_version: 1,
            key_epoch: 1,
            auth_frontier: vec![],
            loro_update: &change.update_bytes,
            semantic_transaction: "stale-stage",
        })
        .unwrap();
    let error = repo
        .commit_local(LocalCommit {
            expected_update_count: 0,
            idempotency_app_id: f.app.id.clone(),
            record,
            document: key,
            update_bytes: change.update_bytes,
            projections: vec![],
            request_hash: [1; 32],
            idempotency_key: "stale-stage".into(),
            response: json!({"durable":true}),
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("state changed before commit"));
    assert!(
        repo.idempotency(&f.app.id, "stale-stage", [1; 32])
            .await
            .unwrap()
            .is_none()
    );
    let changes = f
        .call("document.changes", json!({"community_id":"garden"}))
        .await;
    assert_eq!(changes["changes"].as_array().unwrap().len(), 1);
}
#[tokio::test]
async fn migration_backfills_history_and_cursors_survive_vacuum() {
    let f = Fixture::new().await;
    f.call("document.mutate", edit("old", "old", 0)).await;
    let db = config::database_path(f.temp.path());
    let connection = rusqlite::Connection::open(&db).unwrap();
    connection.execute_batch("DROP TRIGGER document_changes_after_insert; DROP TABLE document_changes; DELETE FROM schema_migrations WHERE version=6;").unwrap();
    sqlite::initialize(&db).unwrap();
    let before = f
        .call("document.changes", json!({"community_id":"garden"}))
        .await;
    assert_eq!(before["changes"].as_array().unwrap().len(), 1);
    connection.execute_batch("VACUUM").unwrap();
    assert_eq!(
        before,
        f.call("document.changes", json!({"community_id":"garden"}))
            .await
    );
    f.call("document.mutate", edit("new", "new", 0)).await;
    let after = f
        .call(
            "document.changes",
            json!({"community_id":"garden","after":before["next_cursor"]}),
        )
        .await;
    assert_eq!(after["changes"].as_array().unwrap().len(), 1);
    assert_eq!(after["changes"][0]["document_id"], "new");
}
#[tokio::test]
async fn planning_notifications_require_current_read_grants() {
    let f = Fixture::new().await;
    for app in [&f.app, &f.other] {
        f.core.call_authenticated(&f.admin,"profile.grant",json!({"app_id":app.id,"profile_id":"community.planning","community_id":"garden","can_read":true,"can_write":true})).await.unwrap();
    }
    let mut event: Value =
        serde_json::from_str(include_str!("fixtures/planning/event-v1.json")).unwrap();
    event["project_id"] = Value::Null;
    event["community_id"] = json!("garden");
    event["idempotency_key"] = json!("event-feed");
    f.call("planning.event.put", event).await;
    let params = json!({"community_id":"garden","profile_id":"community.planning"});
    let feed = f
        .core
        .call_authenticated(&f.other, "document.changes", params.clone())
        .await
        .unwrap();
    assert_eq!(feed["changes"].as_array().unwrap().len(), 1);
    assert!(
        f.call("document.changes", json!({"community_id":"garden"}))
            .await["changes"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    f.core.call_authenticated(&f.admin,"profile.grant",json!({"app_id":f.other.id,"profile_id":"community.planning","community_id":"garden","can_read":false,"can_write":false})).await.unwrap();
    assert!(
        f.core
            .call_authenticated(&f.other, "document.changes", params)
            .await
            .is_err()
    );
}
#[tokio::test]
async fn online_backup_recovers_identity_state_cursors_and_idempotency() {
    let f = Fixture::new().await;
    let request = edit("note", "first", 0);
    let ack = f.call("document.mutate", request.clone()).await;
    let backup = f.temp.path().join("backup");
    recovery::backup(f.temp.path(), &backup).unwrap();
    recovery::verify(&backup).unwrap();
    assert_eq!(
        fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(backup.join("author.ed25519"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert!(recovery::backup(f.temp.path(), &backup).is_err());
    let restored = f.temp.path().join("restored");
    recovery::restore(&backup, &restored).unwrap();
    assert!(recovery::restore(&backup, &restored).is_err());
    let core = make_core(&restored, 77);
    let app = core.authenticate(&f.token).await.unwrap().unwrap();
    assert_eq!(
        f.core.call_public("health").await.unwrap()["author_key"],
        core.call_public("health").await.unwrap()["author_key"]
    );
    assert_eq!(
        core.call_authenticated(&app, "document.mutate", request)
            .await
            .unwrap(),
        ack
    );
    assert_eq!(
        core.call_authenticated(
            &app,
            "document.get",
            json!({"community_id":"garden","document_id":"note"})
        )
        .await
        .unwrap()["state"]["body"],
        "🌱é"
    );
    assert_eq!(
        core.call_authenticated(&app, "document.changes", json!({"community_id":"garden"}))
            .await
            .unwrap()["changes"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    f.call("document.mutate", edit("later", "later", 0)).await;
    recovery::verify(&backup).unwrap();
    fs::write(backup.join("content-master.key"), [0_u8; 32]).unwrap();
    assert!(
        recovery::verify(&backup)
            .unwrap_err()
            .to_string()
            .contains("checksum")
    );
    let refused = f.temp.path().join("refused");
    assert!(recovery::restore(&backup, &refused).is_err());
    assert!(!refused.exists());
    let manifest_path = backup.join("backup.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["files"]["content-master.key"] = json!(blake3::hash(&[0_u8; 32]).to_hex().to_string());
    fs::write(manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(recovery::verify(&backup).is_err());
}
#[test]
fn locks_and_incomplete_recovery_fail_closed() {
    let temp = TempDir::new().unwrap();
    let data = temp.path().join("device");
    let lock = config::lock_data_dir(&data).unwrap();
    assert!(config::lock_data_dir(&data).is_err());
    drop(lock);
    drop(config::lock_data_dir(&data).unwrap());
    fs::write(data.join(recovery::INCOMPLETE), b"").unwrap();
    assert!(config::lock_data_dir(&data).is_err());
    assert!(recovery::verify(&data).is_err());
}
#[test]
fn initialization_cannot_replace_missing_keys_for_existing_history() {
    let temp = TempDir::new().unwrap();
    config::initialize_data_dir(temp.path()).unwrap();
    sqlite::initialize(&config::database_path(temp.path())).unwrap();
    fs::remove_file(temp.path().join("content-master.key")).unwrap();
    assert!(config::initialize_data_dir(temp.path()).is_err());
    assert!(!temp.path().join("content-master.key").exists());
}
