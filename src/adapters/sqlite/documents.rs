use crate::domain::{
    DocumentChangeEntry, DocumentChanges, DocumentChangesPage, DocumentPage, DocumentSummary,
    ListDocuments,
};
use anyhow::Result;
use rusqlite::{Connection, params};
pub(super) fn list(
    connection: &Connection,
    app_id: &str,
    request: &ListDocuments,
) -> Result<DocumentPage> {
    let mut statement = connection.prepare(
        "SELECT document_id,count(*) FROM document_updates
         WHERE app_id=?1 AND community_id=?2 AND applied=1
         AND substr(document_id,1,length(?3))=?3 AND (?4 IS NULL OR document_id>?4)
         GROUP BY document_id ORDER BY document_id LIMIT ?5",
    )?;
    let mut documents = statement
        .query_map(
            params![
                app_id,
                request.community_id,
                request.prefix,
                request.after,
                u32::from(request.limit) + 1
            ],
            |row| {
                Ok(DocumentSummary {
                    document_id: row.get(0)?,
                    revision: nonnegative(row, 1)?,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let more = documents.len() > usize::from(request.limit);
    documents.truncate(usize::from(request.limit));
    let next_cursor = if more {
        documents.last().map(|d| d.document_id.clone())
    } else {
        None
    };
    Ok(DocumentPage {
        documents,
        next_cursor,
    })
}
pub(super) fn changes(
    connection: &Connection,
    app_id: &str,
    request: &DocumentChanges,
) -> Result<DocumentChangesPage> {
    let mut statement = connection.prepare(
        "SELECT change_id,document_id,lower(hex(operation_hash)) FROM document_changes
         WHERE app_id=?1 AND community_id=?2 AND change_id>?3 ORDER BY change_id LIMIT ?4",
    )?;
    let mut changes = statement
        .query_map(
            params![
                app_id,
                request.community_id,
                i64::try_from(request.after)?,
                u32::from(request.limit) + 1
            ],
            |row| {
                Ok(DocumentChangeEntry {
                    cursor: nonnegative(row, 0)?,
                    document_id: row.get(1)?,
                    operation_hash: row.get(2)?,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let has_more = changes.len() > usize::from(request.limit);
    changes.truncate(usize::from(request.limit));
    let next_cursor = changes.last().map_or(request.after, |change| change.cursor);
    Ok(DocumentChangesPage {
        changes,
        next_cursor,
        has_more,
    })
}
fn nonnegative(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(index)?;
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
}
