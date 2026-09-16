#![allow(clippy::too_many_lines)]

use std::{path::Path, sync::Arc};

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
use rusqlite::Connection;
use serde_json::{Value, json};
use tempfile::TempDir;

#[test]
fn canonical_profile_artifact_matches_pinned_digest() {
    let bytes = include_bytes!("../protocol/profiles/community-planning-v1.json");
    assert_eq!(
        blake3::hash(bytes).to_hex().as_str(),
        community_stack::domain::PLANNING_PROFILE_DIGEST
    );
}

fn make_core(db: &Path, data: &Path) -> CommunityCore {
    let (signing_key, master_key) = config::load_keys(data).unwrap();
    CommunityCore::new(
        Arc::new(StoreHandle::start(db).unwrap()),
        Arc::new(LoroDocumentEngine),
        Arc::new(P2pandaSecureLog::new(signing_key, master_key)),
        Arc::new(Blake3ContentHasher),
        4_242,
    )
}

async fn setup() -> (
    TempDir,
    CommunityCore,
    Principal,
    Principal,
    Principal,
    Principal,
) {
    let temp = TempDir::new().unwrap();
    config::initialize_data_dir(temp.path()).unwrap();
    let db = config::database_path(temp.path());
    let mut tokens = Vec::new();
    for (id, role) in [
        ("org.example.calendar", "APP"),
        ("org.example.gantt", "APP"),
        ("org.example.ungranted", "APP"),
        ("device-admin", "ADMIN"),
    ] {
        let (hash, token) = config::generate_token().unwrap();
        sqlite::register_application(&db, id, &hash, role).unwrap();
        tokens.push(token);
    }
    let core = make_core(&db, temp.path());
    let calendar = core.authenticate(&tokens[0]).await.unwrap().unwrap();
    let gantt = core.authenticate(&tokens[1]).await.unwrap().unwrap();
    let ungranted = core.authenticate(&tokens[2]).await.unwrap().unwrap();
    let admin = core.authenticate(&tokens[3]).await.unwrap().unwrap();
    (temp, core, calendar, gantt, ungranted, admin)
}

async fn call(core: &CommunityCore, principal: &Principal, method: &str, params: Value) -> Value {
    core.call_authenticated(principal, method, params)
        .await
        .unwrap()
}

async fn grant(core: &CommunityCore, admin: &Principal, app_id: &str, write: bool) {
    call(
        core,
        admin,
        "profile.grant",
        json!({
            "app_id":app_id,
            "profile_id":"community.planning",
            "community_id":"team",
            "can_read":true,
            "can_write":write
        }),
    )
    .await;
}

#[test]
fn legacy_reserved_planning_principal_blocks_profile_upgrade() {
    let temp = TempDir::new().unwrap();
    for role in ["APP", "ADMIN"] {
        let db = temp.path().join(format!("reserved-{role}.sqlite3"));
        let connection = Connection::open(&db).unwrap();
        for (version, sql) in [
            (1_i64, include_str!("../migrations/0001_core.sql")),
            (2_i64, include_str!("../migrations/0002_toolkit.sql")),
            (
                3_i64,
                include_str!("../migrations/0003_fact_governance.sql"),
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
        let sql = if role == "APP" {
            "INSERT INTO applications(app_id,token_hash,role,enabled,created_at_ms) VALUES('community.planning',?1,'APP',1,1)"
        } else {
            "INSERT INTO administrators(principal_id,token_hash,enabled,created_at_ms) VALUES('community.planning',?1,1,1)"
        };
        connection.execute(sql, [[4_u8; 32].as_slice()]).unwrap();
        drop(connection);
        let error = sqlite::initialize(&db).unwrap_err().to_string();
        assert!(error.contains("reserved namespace"), "{role}: {error}");
        assert!(error.contains("previous release"), "{role}: {error}");
    }
}

#[test]
fn version_three_database_adds_profile_tables_without_rewriting_operations() {
    let temp = TempDir::new().unwrap();
    let db = temp.path().join("v3.sqlite3");
    let connection = Connection::open(&db).unwrap();
    for (version, sql) in [
        (1_i64, include_str!("../migrations/0001_core.sql")),
        (2_i64, include_str!("../migrations/0002_toolkit.sql")),
        (
            3_i64,
            include_str!("../migrations/0003_fact_governance.sql"),
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
    let hash = [9_u8; 32];
    connection.execute(
        "INSERT INTO operations(operation_hash,canonical_header,body_ciphertext,author_key,log_id,generation,sequence,backlink,app_id,community_id,document_id,record_kind,verification_status,apply_status,received_at_ms) VALUES(?1,?2,?3,?4,?5,0,0,NULL,'legacy.app','legacy','legacy/document',1,'VERIFIED','APPLIED',1)",
        rusqlite::params![hash.as_slice(), b"unchanged-header", b"unchanged-body", [8_u8;32].as_slice(), [7_u8;32].as_slice()],
    ).unwrap();
    drop(connection);

    sqlite::initialize(&db).unwrap();
    sqlite::initialize(&db).unwrap();
    let upgraded = Connection::open(&db).unwrap();
    let bytes: (Vec<u8>, Vec<u8>) = upgraded
        .query_row(
            "SELECT canonical_header,body_ciphertext FROM operations WHERE operation_hash=?1",
            [hash.as_slice()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        bytes,
        (b"unchanged-header".to_vec(), b"unchanged-body".to_vec())
    );
    let versions: i64 = upgraded
        .query_row("SELECT count(*) FROM schema_migrations", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(versions, 6);
    let planning_tables: i64 = upgraded
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name LIKE 'planning_%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(planning_tables, 5);
}

#[tokio::test(flavor = "multi_thread")]
async fn independent_apps_share_one_profile_namespace_and_views() {
    let (temp, core, calendar, gantt, ungranted, admin) = setup().await;
    let db = config::database_path(temp.path());

    let denied = core
        .call_authenticated(
            &ungranted,
            "planning.calendar.list",
            json!({"community_id":"team"}),
        )
        .await
        .unwrap_err();
    assert!(denied.to_string().contains("not granted"));

    grant(&core, &admin, "org.example.calendar", true).await;
    grant(&core, &admin, "org.example.gantt", true).await;
    assert!(
        core.call_authenticated(
            &calendar,
            "planning.calendar.list",
            json!({"community_id":"another-community"}),
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("not granted")
    );
    let profile = call(
        &core,
        &calendar,
        "profile.get",
        json!({"profile_id":"community.planning","profile_version":1,"community_id":"team"}),
    )
    .await;
    assert_eq!(profile["data_namespace"], "community.planning");
    assert_eq!(profile["access"]["can_write"], true);
    assert_eq!(
        profile["profile_digest"],
        "771d32c6a851318c84139c924c573eac69d9ab8c7edca4f179afe344c48879ee"
    );
    let canonical_contract: Value = serde_json::from_str(include_str!(
        "../protocol/profiles/community-planning-v1.json"
    ))
    .unwrap();
    assert_eq!(profile["contract"], canonical_contract);

    let project_params = json!({
        "community_id":"team", "idempotency_key":"project-1",
        "project_id":"release", "title":"Release", "description":"Shared plan",
        "status":"active"
    });
    let project = call(
        &core,
        &calendar,
        "planning.project.put",
        project_params.clone(),
    )
    .await;
    let project_retry = call(&core, &calendar, "planning.project.put", project_params).await;
    assert_eq!(project, project_retry);
    let signed_project: Value = serde_json::from_str(
        project["state"]["record"]["canonical_json"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(signed_project["profile_id"], "community.planning");
    assert_eq!(signed_project["profile_digest"], profile["profile_digest"]);
    assert_eq!(
        signed_project["submitted_by_app_id"],
        "org.example.calendar"
    );
    assert!(
        project["state"]["record"]["canonical_json"]
            .as_str()
            .unwrap()
            .contains(&format!(
                "\"record\":{}",
                include_str!("fixtures/planning/project-v1.json")
            ))
    );
    assert!(
        signed_project["record"]
            .get("source_operation_hash")
            .is_none()
    );

    let design_response = call(
        &core,
        &calendar,
        "planning.task.put",
        json!({
            "community_id":"team", "idempotency_key":"task-design",
            "task_id":"design", "project_id":"release", "title":"Design",
            "status":"completed", "start_at_ms":1000, "due_at_ms":2000,
            "progress_percent":100
        }),
    )
    .await;
    let signed_design: Value = serde_json::from_str(
        design_response["state"]["record"]["canonical_json"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert!(
        signed_design["record"]
            .get("source_operation_hash")
            .is_none()
    );
    assert!(
        design_response["state"]["record"]["canonical_json"]
            .as_str()
            .unwrap()
            .contains(&format!(
                "\"record\":{}",
                include_str!("fixtures/planning/task-v1.json")
            ))
    );
    call(
        &core,
        &gantt,
        "planning.task.put",
        json!({
            "community_id":"team", "idempotency_key":"task-build",
            "task_id":"build", "project_id":"release", "title":"Build",
            "status":"active", "start_at_ms":2000, "due_at_ms":5000,
            "progress_percent":40
        }),
    )
    .await;
    let dependency_response = call(
        &core,
        &gantt,
        "planning.dependency.put",
        json!({
            "community_id":"team", "idempotency_key":"dependency-1",
            "dependency_id":"design-before-build", "predecessor_task_id":"design",
            "successor_task_id":"build", "kind":"finish_to_start", "lag_ms":0
        }),
    )
    .await;
    let signed_dependency: Value = serde_json::from_str(
        dependency_response["state"]["record"]["canonical_json"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert!(
        signed_dependency["record"]
            .get("source_operation_hash")
            .is_none()
    );
    assert!(
        dependency_response["state"]["record"]["canonical_json"]
            .as_str()
            .unwrap()
            .contains(&format!(
                "\"record\":{}",
                include_str!("fixtures/planning/dependency-v1.json")
            ))
    );
    let event_response = call(
        &core,
        &calendar,
        "planning.event.put",
        json!({
            "community_id":"team", "idempotency_key":"event-1",
            "event_id":"standup", "project_id":"release", "title":"Stand-up",
            "status":"active",
            "timing":{"kind":"utc","start_at_ms":10000,"end_at_ms":11800,"time_zone":"Europe/Berlin"},
            "recurrence":{"frequency":"weekly","interval":1,"count":8,"by_weekday":["monday"]},
            "location":"Room 4"
        }),
    )
    .await;
    let signed_event: Value = serde_json::from_str(
        event_response["state"]["record"]["canonical_json"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert!(
        signed_event["record"]
            .get("source_operation_hash")
            .is_none()
    );
    assert!(
        event_response["state"]["record"]["canonical_json"]
            .as_str()
            .unwrap()
            .contains(&format!(
                "\"record\":{}",
                include_str!("fixtures/planning/event-v1.json")
            ))
    );

    let gantt_view = call(
        &core,
        &calendar,
        "planning.gantt.get",
        json!({"community_id":"team","limit":100}),
    )
    .await;
    assert_eq!(gantt_view["projects"].as_array().unwrap().len(), 1);
    assert_eq!(gantt_view["tasks"].as_array().unwrap().len(), 2);
    assert_eq!(gantt_view["dependencies"].as_array().unwrap().len(), 1);
    assert!(
        gantt_view["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|task| task["submitted_by_app_id"] == "org.example.gantt")
    );

    let calendar_view = call(
        &core,
        &gantt,
        "planning.calendar.list",
        json!({"community_id":"team","limit":100}),
    )
    .await;
    assert_eq!(calendar_view["events"].as_array().unwrap().len(), 1);
    assert_eq!(calendar_view["events"][0]["event_id"], "standup");
    assert_eq!(
        calendar_view["events"][0]["recurrence"]["frequency"],
        "weekly"
    );
    assert_eq!(
        calendar_view["profile"]["profile_digest"],
        profile["profile_digest"]
    );

    // Generic APP document access remains app-scoped and cannot bypass the
    // explicit shared-profile capability boundary.
    let generic = call(
        &core,
        &calendar,
        "document.get",
        json!({"community_id":"team","document_id":"planning/tasks/build"}),
    )
    .await;
    assert_eq!(generic["update_count"], 0);

    let reader = Connection::open(&db).unwrap();
    let namespaces: i64 = reader
        .query_row(
            "SELECT count(DISTINCT app_id) FROM operations WHERE document_id LIKE 'planning/%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(namespaces, 1);
    let namespace: String = reader
        .query_row(
            "SELECT app_id FROM operations WHERE document_id LIKE 'planning/%' LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(namespace, "community.planning");
    let project_operations: i64 = reader
        .query_row(
            "SELECT count(*) FROM operations WHERE document_id='planning/projects/release'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(project_operations, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn planning_grant_is_rechecked_inside_the_commit_transaction() {
    let (temp, core, calendar, _, _, admin) = setup().await;
    let db = config::database_path(temp.path());
    grant(&core, &admin, "org.example.calendar", true).await;
    Connection::open(&db)
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER revoke_during_planning_commit BEFORE INSERT ON operations
             WHEN NEW.document_id='planning/projects/revoked'
             BEGIN
               DELETE FROM profile_grants
               WHERE app_id='org.example.calendar'
                 AND profile_id='community.planning'
                 AND community_id='team';
             END;",
        )
        .unwrap();
    let error = core
        .call_authenticated(
            &calendar,
            "planning.project.put",
            json!({
                "community_id":"team","idempotency_key":"revoked-write",
                "project_id":"revoked","title":"Must roll back","status":"active"
            }),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("revoked before commit"));
    let operations: i64 = Connection::open(&db)
        .unwrap()
        .query_row("SELECT count(*) FROM operations", [], |row| row.get(0))
        .unwrap();
    assert_eq!(operations, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn profile_validation_and_dependency_policy_fail_closed() {
    let (temp, core, calendar, _, _, admin) = setup().await;
    let db = config::database_path(temp.path());
    grant(&core, &admin, "org.example.calendar", true).await;
    let grant_denied = core
        .call_authenticated(
            &calendar,
            "profile.grant",
            json!({"app_id":"org.example.calendar","profile_id":"community.planning","community_id":"team","can_read":true,"can_write":true}),
        )
        .await
        .unwrap_err();
    assert!(grant_denied.to_string().contains("not allowed"));

    let before: i64 = Connection::open(&db)
        .unwrap()
        .query_row("SELECT count(*) FROM operations", [], |row| row.get(0))
        .unwrap();
    let invalid = core
        .call_authenticated(
            &calendar,
            "planning.event.put",
            json!({
                "community_id":"team", "idempotency_key":"invalid-event",
                "event_id":"bad", "title":"Bad date", "status":"planned",
                "timing":{"kind":"all_day","start_date":"2026-02-30","end_date_exclusive":"2026-03-02"}
            }),
        )
        .await
        .unwrap_err();
    assert!(invalid.to_string().contains("invalid date"));
    let after: i64 = Connection::open(&db)
        .unwrap()
        .query_row("SELECT count(*) FROM operations", [], |row| row.get(0))
        .unwrap();
    assert_eq!(before, after);

    call(
        &core,
        &calendar,
        "planning.task.put",
        json!({"community_id":"team","idempotency_key":"a","task_id":"a","title":"A","status":"active"}),
    )
    .await;
    call(
        &core,
        &calendar,
        "planning.task.put",
        json!({"community_id":"team","idempotency_key":"b","task_id":"b","title":"B","status":"active"}),
    )
    .await;
    call(
        &core,
        &calendar,
        "planning.dependency.put",
        json!({"community_id":"team","idempotency_key":"a-b","dependency_id":"a-b","predecessor_task_id":"a","successor_task_id":"b","kind":"finish_to_start"}),
    )
    .await;
    let cycle = core
        .call_authenticated(
            &calendar,
            "planning.dependency.put",
            json!({"community_id":"team","idempotency_key":"b-a","dependency_id":"b-a","predecessor_task_id":"b","successor_task_id":"a","kind":"finish_to_start"}),
        )
        .await
        .unwrap_err();
    assert!(cycle.to_string().contains("cycle"));
    call(
        &core,
        &calendar,
        "planning.task.put",
        json!({"community_id":"team","idempotency_key":"c","task_id":"c","title":"C","status":"active"}),
    )
    .await;
    call(
        &core,
        &calendar,
        "planning.task.put",
        json!({"community_id":"team","idempotency_key":"d","task_id":"d","title":"D","status":"active"}),
    )
    .await;
    let second_core = make_core(&db, temp.path());
    let (c_to_d, d_to_c) = tokio::join!(
        core.call_authenticated(
            &calendar,
            "planning.dependency.put",
            json!({"community_id":"team","idempotency_key":"c-d","dependency_id":"c-d","predecessor_task_id":"c","successor_task_id":"d","kind":"finish_to_start"}),
        ),
        second_core.call_authenticated(
            &calendar,
            "planning.dependency.put",
            json!({"community_id":"team","idempotency_key":"d-c","dependency_id":"d-c","predecessor_task_id":"d","successor_task_id":"c","kind":"finish_to_start"}),
        )
    );
    assert_ne!(c_to_d.is_ok(), d_to_c.is_ok());
    let concurrent_error = c_to_d.err().or_else(|| d_to_c.err()).unwrap();
    assert!(
        concurrent_error.to_string().contains("cycle")
            || concurrent_error.to_string().contains("state changed")
    );

    let dependencies: i64 = Connection::open(&db)
        .unwrap()
        .query_row("SELECT count(*) FROM planning_dependencies", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(dependencies, 2);
}
