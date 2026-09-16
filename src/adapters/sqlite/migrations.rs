use super::now_ms;
use anyhow::{Result, bail};
use rusqlite::{Connection, TransactionBehavior, params};
const MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("../../../migrations/0001_core.sql")),
    (2, include_str!("../../../migrations/0002_toolkit.sql")),
    (
        3,
        include_str!("../../../migrations/0003_fact_governance.sql"),
    ),
    (
        4,
        include_str!("../../../migrations/0004_planning_profile.sql"),
    ),
    (
        5,
        include_str!("../../../migrations/0005_authorization_and_invariants.sql"),
    ),
    (
        6,
        include_str!("../../../migrations/0006_document_changes.sql"),
    ),
];
const DIGESTS: [&str; 6] = [
    "9bbdbdb32bfa205624db23724fb13b10c1ba0c8b19b549c66153b8b683c2b2c0",
    "a07e36958351e4633e8fe978cd213a11d97417512c4b1e139d1f415ad0fb0bb5",
    "c6a1ff0c8368f95a301a6e2ca1048d0cdedac55c462463b0331521df3b189cd6",
    "6d7d7893d1836c94fbfdd3720d88bfb0fc7cb987131963e304248feabfa5a3f1",
    "241a9e34078d5a60998c15dc99cce9f35048c059a5a6e44714ca898415126963",
    "2ab18adf93cdfb8aefb10c23aca0af0ab1972d3372d4695757c25e3f84d53dc8",
];
pub(super) fn apply_migrations(connection: &mut Connection) -> Result<()> {
    for ((version, sql), expected) in MIGRATIONS.iter().zip(DIGESTS) {
        if blake3::hash(sql.as_bytes()).to_hex().as_str() != expected {
            bail!("embedded migration {version} does not match its reviewed digest");
        }
    }

    let migration_table_exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_migrations')",
        [],
        |row| row.get(0),
    )?;
    if migration_table_exists {
        let newest: Option<i64> =
            connection.query_row("SELECT max(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })?;
        if newest.is_some_and(|version| version > 6) {
            bail!("database was created by a newer unsupported migration version");
        }
    }

    for (version, sql) in MIGRATIONS {
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let table_exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_migrations')",
            [],
            |row| row.get(0),
        )?;
        let applied = table_exists
            && tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version=?1)",
                [version],
                |row| row.get(0),
            )?;
        if !applied {
            if *version == 4 {
                preflight_v4_upgrade(&tx)?;
            } else if *version == 5 {
                preflight_v5_upgrade(&tx)?;
            }
            if *version == 6 {
                verify_schema_prefix(&tx, 5)?;
            }
            tx.execute_batch(sql)?;
            tx.execute(
                "INSERT INTO schema_migrations(version,applied_at_ms) VALUES(?1,?2)",
                params![version, now_ms()?],
            )?;
            if *version == 5 {
                for ((known_version, _), digest) in MIGRATIONS.iter().zip(DIGESTS) {
                    tx.execute(
                        "UPDATE schema_migrations SET checksum=?1 WHERE version=?2",
                        params![digest, known_version],
                    )?;
                }
            } else if *version > 5 {
                tx.execute(
                    "UPDATE schema_migrations SET checksum=?1 WHERE version=?2",
                    params![DIGESTS[usize::try_from(*version - 1)?], version],
                )?;
            }
        }
        tx.commit()?;
    }

    verify_complete_schema(connection)
}
pub(super) fn verify_complete_schema(connection: &Connection) -> Result<()> {
    verify_schema_prefix(connection, MIGRATIONS.len())
}
fn verify_schema_prefix(connection: &Connection, count: usize) -> Result<()> {
    let mut statement =
        connection.prepare("SELECT version,checksum FROM schema_migrations ORDER BY version")?;
    let stored = statement
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if stored.len() != count {
        bail!("database migration set is incomplete or unsupported");
    }
    for ((version, digest), ((expected_version, _), expected_digest)) in
        stored.iter().zip(MIGRATIONS.iter().zip(DIGESTS))
    {
        if version != expected_version || digest.as_deref() != Some(expected_digest) {
            bail!("database migration checksum mismatch at version {version}");
        }
    }
    Ok(())
}

fn preflight_v4_upgrade(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    let collision: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM applications WHERE app_id=?1 AND enabled=1) OR EXISTS(SELECT 1 FROM administrators WHERE principal_id=?1 AND enabled=1)",
        [crate::domain::PLANNING_NAMESPACE],
        |row| row.get(0),
    )?;
    if collision {
        bail!(
            "legacy principal uses reserved namespace community.planning; disable or rename it with the previous release before upgrading"
        );
    }
    Ok(())
}

fn preflight_v5_upgrade(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    for table in [
        "applications",
        "administrators",
        "operations",
        "document_updates",
        "toolkit_concepts",
        "fact_claim_revisions",
        "planning_projects",
        "profile_grants",
    ] {
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            [table],
            |row| row.get(0),
        )?;
        if !exists {
            bail!("legacy migration state is missing required table {table}");
        }
    }
    let ambiguous: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM applications a JOIN administrators d ON a.app_id=d.principal_id OR a.token_hash=d.token_hash)",
        [],
        |row| row.get(0),
    )?;
    if ambiguous {
        bail!(
            "legacy APP/ADMIN credentials are ambiguous; remove or rotate the duplicate while running the previous release, then retry"
        );
    }
    Ok(())
}
