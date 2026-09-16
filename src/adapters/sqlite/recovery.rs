use super::migrations;
use crate::domain::DocumentKey;
use anyhow::{Result, bail};
use rusqlite::Connection;
use std::{path::Path, thread, time::Duration};

pub fn backup_database(source: &Path, destination: &Path) -> Result<()> {
    let source = Connection::open_with_flags(source, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    source.busy_timeout(Duration::from_secs(5))?;
    let mut destination_connection = Connection::open(destination)?;
    {
        let backup = rusqlite::backup::Backup::new(&source, &mut destination_connection)?;
        let started = std::time::Instant::now();
        loop {
            if started.elapsed() > Duration::from_secs(60) {
                bail!("online backup exceeded its time budget; retry during lower write activity");
            }
            match backup.step(256)? {
                rusqlite::backup::StepResult::Done => break,
                rusqlite::backup::StepResult::More => {}
                rusqlite::backup::StepResult::Busy | rusqlite::backup::StepResult::Locked => {
                    thread::sleep(Duration::from_millis(10));
                }
                _ => bail!("unsupported SQLite backup state"),
            }
        }
    }
    destination_connection.pragma_update(None, "journal_mode", "DELETE")?;
    drop(destination_connection);
    verify_backup_database(destination)?;
    std::fs::File::open(destination)?.sync_all()?;
    Ok(())
}
pub fn verify_backup_database(path: &Path) -> Result<()> {
    let connection = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let result: String = connection.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if result != "ok" {
        bail!("backup database integrity check failed");
    }
    let violations: i64 =
        connection.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })?;
    if violations != 0 {
        bail!("backup database has foreign key violations");
    }
    migrations::verify_complete_schema(&connection)
}
pub fn visit_backup_updates<F>(path: &Path, mut inspect: F) -> Result<()>
where
    F: FnMut(&DocumentKey, &[u8], &[u8], &[u8], &[u8]) -> Result<()>,
{
    let connection = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let missing: i64 = connection.query_row(
        "SELECT count(*) FROM operations o LEFT JOIN document_updates d USING(operation_hash)
         WHERE o.apply_status='APPLIED' AND (d.operation_hash IS NULL OR d.applied!=1)",
        [],
        |row| row.get(0),
    )?;
    if missing != 0 {
        bail!("backup has applied operations without document updates");
    }
    let mut statement = connection.prepare(
        "SELECT d.app_id,d.community_id,d.document_id,o.operation_hash,o.canonical_header,o.body_ciphertext,d.update_bytes,
         length(o.canonical_header),length(o.body_ciphertext),length(d.update_bytes),o.app_id,o.community_id,o.document_id
         FROM document_updates d JOIN operations o USING(operation_hash) WHERE d.applied=1")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        for index in 7..=9 {
            let bytes: i64 = row.get(index)?;
            if !(1..=1_048_576).contains(&bytes) {
                bail!("backup record exceeds supported size");
            }
        }
        let key = DocumentKey {
            app_id: row.get(0)?,
            community_id: row.get(1)?,
            document_id: row.get(2)?,
        };
        if key.app_id != row.get::<_, String>(10)?
            || key.community_id != row.get::<_, String>(11)?
            || key.document_id != row.get::<_, String>(12)?
        {
            bail!("backup operation/document linkage mismatch");
        }
        let hash: Vec<u8> = row.get(3)?;
        let header: Vec<u8> = row.get(4)?;
        let body: Vec<u8> = row.get(5)?;
        let update: Vec<u8> = row.get(6)?;
        inspect(&key, &hash, &header, &body, &update)?;
    }
    Ok(())
}
