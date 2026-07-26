use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, Transaction, params, types::Type};

use crate::domain::{
    DependencyKind, PlanningCatalog, PlanningDependency, PlanningEvent, PlanningProject,
    PlanningProjection, PlanningProjectionWrite, PlanningStatus, PlanningTask, ProfileAccess,
};

use super::now_ms;

pub(super) fn profile_access(
    connection: &Connection,
    app_id: &str,
    profile_id: &str,
    community_id: &str,
) -> Result<ProfileAccess> {
    connection
        .query_row(
            "SELECT can_read,can_write FROM profile_grants WHERE app_id=?1 AND profile_id=?2 AND community_id=?3",
            params![app_id, profile_id, community_id],
            |row| {
                Ok(ProfileAccess {
                    can_read: row.get(0)?,
                    can_write: row.get(1)?,
                })
            },
        )
        .or_else(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => Ok(ProfileAccess::default()),
            error => Err(error),
        })
        .map_err(Into::into)
}

pub(super) fn grant_profile(
    connection: &mut Connection,
    app_id: &str,
    profile_id: &str,
    community_id: &str,
    access: ProfileAccess,
) -> Result<()> {
    let now = now_ms()?;
    let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    tx.execute(
        "INSERT INTO profile_grants(app_id,profile_id,community_id,can_read,can_write,granted_at_ms) VALUES(?1,?2,?3,?4,?5,?6) \
         ON CONFLICT(app_id,profile_id,community_id) DO UPDATE SET can_read=excluded.can_read,can_write=excluded.can_write,granted_at_ms=excluded.granted_at_ms",
        params![app_id, profile_id, community_id, i64::from(access.can_read), i64::from(access.can_write), now],
    )?;
    tx.execute(
        "INSERT INTO audit_events(event_kind,app_id,community_id,object_id,redacted_detail,created_at_ms) VALUES('PROFILE_GRANT_CHANGED',?1,?2,?3,'local profile capability changed',?4)",
        params![app_id, community_id, profile_id, now],
    )?;
    tx.commit()?;
    Ok(())
}

pub(super) fn apply_projection(
    tx: &Transaction<'_>,
    namespace_id: &str,
    community_id: &str,
    operation_hash: &[u8; 32],
    write: PlanningProjectionWrite,
    now: i64,
) -> Result<()> {
    let PlanningProjectionWrite {
        profile_digest: digest,
        authorized_app_id,
        profile_id,
        expected_community_version,
        projection,
    } = write;
    let authorized: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM profile_grants WHERE app_id=?1 AND profile_id=?2 AND community_id=?3 AND can_read=1 AND can_write=1)",
        params![authorized_app_id, profile_id, community_id],
        |row| row.get(0),
    )?;
    if !authorized {
        anyhow::bail!("planning profile grant was revoked before commit");
    }
    let expected_version = i64::try_from(expected_community_version)?;
    let current_version = tx
        .query_row(
            "SELECT version FROM planning_community_versions WHERE namespace_id=?1 AND community_id=?2",
            params![namespace_id, community_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .unwrap_or(0);
    if current_version != expected_version {
        anyhow::bail!("planning state changed before commit");
    }
    tx.execute(
        "INSERT INTO planning_community_versions(namespace_id,community_id,version) VALUES(?1,?2,1) \
         ON CONFLICT(namespace_id,community_id) DO UPDATE SET version=version+1",
        params![namespace_id, community_id],
    )?;
    match projection {
        PlanningProjection::Project(project) => {
            tx.execute(
                "INSERT INTO planning_projects(namespace_id,community_id,project_id,title,description,status,profile_digest,submitted_by_app_id,source_operation_hash,updated_at_ms) \
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT(namespace_id,community_id,project_id) DO UPDATE SET title=excluded.title,description=excluded.description,status=excluded.status,profile_digest=excluded.profile_digest,submitted_by_app_id=excluded.submitted_by_app_id,source_operation_hash=excluded.source_operation_hash,updated_at_ms=excluded.updated_at_ms",
                params![namespace_id,community_id,project.project_id,project.title,project.description,project.status.as_str(),digest,project.submitted_by_app_id,operation_hash.as_slice(),now],
            )?;
        }
        PlanningProjection::Task(task) => {
            tx.execute(
                "INSERT INTO planning_tasks(namespace_id,community_id,task_id,project_id,title,description,status,start_at_ms,due_at_ms,progress_percent,profile_digest,submitted_by_app_id,source_operation_hash,updated_at_ms) \
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14) ON CONFLICT(namespace_id,community_id,task_id) DO UPDATE SET project_id=excluded.project_id,title=excluded.title,description=excluded.description,status=excluded.status,start_at_ms=excluded.start_at_ms,due_at_ms=excluded.due_at_ms,progress_percent=excluded.progress_percent,profile_digest=excluded.profile_digest,submitted_by_app_id=excluded.submitted_by_app_id,source_operation_hash=excluded.source_operation_hash,updated_at_ms=excluded.updated_at_ms",
                params![namespace_id,community_id,task.task_id,task.project_id,task.title,task.description,task.status.as_str(),task.start_at_ms,task.due_at_ms,i64::from(task.progress_percent),digest,task.submitted_by_app_id,operation_hash.as_slice(),now],
            )?;
        }
        PlanningProjection::Event(event) => {
            tx.execute(
                "INSERT INTO planning_events(namespace_id,community_id,event_id,project_id,title,description,status,timing_json,recurrence_json,location,profile_digest,submitted_by_app_id,source_operation_hash,updated_at_ms) \
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14) ON CONFLICT(namespace_id,community_id,event_id) DO UPDATE SET project_id=excluded.project_id,title=excluded.title,description=excluded.description,status=excluded.status,timing_json=excluded.timing_json,recurrence_json=excluded.recurrence_json,location=excluded.location,profile_digest=excluded.profile_digest,submitted_by_app_id=excluded.submitted_by_app_id,source_operation_hash=excluded.source_operation_hash,updated_at_ms=excluded.updated_at_ms",
                params![namespace_id,community_id,event.event_id,event.project_id,event.title,event.description,event.status.as_str(),serde_json::to_vec(&event.timing)?,event.recurrence.as_ref().map(serde_json::to_vec).transpose()?,event.location,digest,event.submitted_by_app_id,operation_hash.as_slice(),now],
            )?;
        }
        PlanningProjection::Dependency(dependency) => {
            tx.execute(
                "INSERT INTO planning_dependencies(namespace_id,community_id,dependency_id,predecessor_task_id,successor_task_id,dependency_kind,lag_ms,profile_digest,submitted_by_app_id,source_operation_hash,updated_at_ms) \
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11) ON CONFLICT(namespace_id,community_id,dependency_id) DO UPDATE SET predecessor_task_id=excluded.predecessor_task_id,successor_task_id=excluded.successor_task_id,dependency_kind=excluded.dependency_kind,lag_ms=excluded.lag_ms,profile_digest=excluded.profile_digest,submitted_by_app_id=excluded.submitted_by_app_id,source_operation_hash=excluded.source_operation_hash,updated_at_ms=excluded.updated_at_ms",
                params![namespace_id,community_id,dependency.dependency_id,dependency.predecessor_task_id,dependency.successor_task_id,dependency.kind.as_str(),dependency.lag_ms,digest,dependency.submitted_by_app_id,operation_hash.as_slice(),now],
            )?;
        }
    }
    Ok(())
}

pub(super) fn catalog(
    connection: &Connection,
    namespace_id: &str,
    community_id: &str,
    limit: u16,
) -> Result<PlanningCatalog> {
    let limit = i64::from(limit.min(10_001));
    let version = connection
        .query_row(
            "SELECT version FROM planning_community_versions WHERE namespace_id=?1 AND community_id=?2",
            params![namespace_id, community_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .map_or(Ok(0), u64::try_from)?;
    Ok(PlanningCatalog {
        version,
        projects: projects(connection, namespace_id, community_id, limit)?,
        tasks: tasks(connection, namespace_id, community_id, limit)?,
        events: events(connection, namespace_id, community_id, limit)?,
        dependencies: dependencies(connection, namespace_id, community_id, limit)?,
    })
}

fn projects(
    connection: &Connection,
    namespace_id: &str,
    community_id: &str,
    limit: i64,
) -> Result<Vec<PlanningProject>> {
    let mut statement = connection.prepare(
        "SELECT project_id,title,description,status,submitted_by_app_id,lower(hex(source_operation_hash)) FROM planning_projects WHERE namespace_id=?1 AND community_id=?2 ORDER BY project_id LIMIT ?3",
    )?;
    collect(
        statement.query_map(params![namespace_id, community_id, limit], |row| {
            Ok(PlanningProject {
                project_id: row.get(0)?,
                title: row.get(1)?,
                description: row.get(2)?,
                status: status(row.get::<_, String>(3)?.as_str())?,
                submitted_by_app_id: row.get(4)?,
                source_operation_hash: row.get(5)?,
            })
        })?,
    )
}

fn tasks(
    connection: &Connection,
    namespace_id: &str,
    community_id: &str,
    limit: i64,
) -> Result<Vec<PlanningTask>> {
    let mut statement = connection.prepare(
        "SELECT task_id,project_id,title,description,status,start_at_ms,due_at_ms,progress_percent,submitted_by_app_id,lower(hex(source_operation_hash)) FROM planning_tasks WHERE namespace_id=?1 AND community_id=?2 ORDER BY task_id LIMIT ?3",
    )?;
    collect(
        statement.query_map(params![namespace_id, community_id, limit], |row| {
            Ok(PlanningTask {
                task_id: row.get(0)?,
                project_id: row.get(1)?,
                title: row.get(2)?,
                description: row.get(3)?,
                status: status(row.get::<_, String>(4)?.as_str())?,
                start_at_ms: row.get(5)?,
                due_at_ms: row.get(6)?,
                progress_percent: row.get(7)?,
                submitted_by_app_id: row.get(8)?,
                source_operation_hash: row.get(9)?,
            })
        })?,
    )
}

fn events(
    connection: &Connection,
    namespace_id: &str,
    community_id: &str,
    limit: i64,
) -> Result<Vec<PlanningEvent>> {
    let mut statement = connection.prepare(
        "SELECT event_id,project_id,title,description,status,timing_json,recurrence_json,location,submitted_by_app_id,lower(hex(source_operation_hash)) FROM planning_events WHERE namespace_id=?1 AND community_id=?2 ORDER BY event_id LIMIT ?3",
    )?;
    collect(
        statement.query_map(params![namespace_id, community_id, limit], |row| {
            let timing: Vec<u8> = row.get(5)?;
            let recurrence: Option<Vec<u8>> = row.get(6)?;
            Ok(PlanningEvent {
                event_id: row.get(0)?,
                project_id: row.get(1)?,
                title: row.get(2)?,
                description: row.get(3)?,
                status: status(row.get::<_, String>(4)?.as_str())?,
                timing: json_value(&timing, 5)?,
                recurrence: recurrence
                    .as_deref()
                    .map(|bytes| json_value(bytes, 6))
                    .transpose()?,
                location: row.get(7)?,
                submitted_by_app_id: row.get(8)?,
                source_operation_hash: row.get(9)?,
            })
        })?,
    )
}

fn dependencies(
    connection: &Connection,
    namespace_id: &str,
    community_id: &str,
    limit: i64,
) -> Result<Vec<PlanningDependency>> {
    let mut statement = connection.prepare(
        "SELECT dependency_id,predecessor_task_id,successor_task_id,dependency_kind,lag_ms,submitted_by_app_id,lower(hex(source_operation_hash)) FROM planning_dependencies WHERE namespace_id=?1 AND community_id=?2 ORDER BY dependency_id LIMIT ?3",
    )?;
    collect(
        statement.query_map(params![namespace_id, community_id, limit], |row| {
            Ok(PlanningDependency {
                dependency_id: row.get(0)?,
                predecessor_task_id: row.get(1)?,
                successor_task_id: row.get(2)?,
                kind: dependency_kind(row.get::<_, String>(3)?.as_str())?,
                lag_ms: row.get(4)?,
                submitted_by_app_id: row.get(5)?,
                source_operation_hash: row.get(6)?,
            })
        })?,
    )
}

fn status(value: &str) -> rusqlite::Result<PlanningStatus> {
    match value {
        "planned" => Ok(PlanningStatus::Planned),
        "active" => Ok(PlanningStatus::Active),
        "completed" => Ok(PlanningStatus::Completed),
        "cancelled" => Ok(PlanningStatus::Cancelled),
        "archived" => Ok(PlanningStatus::Archived),
        _ => conversion_error("invalid planning status"),
    }
}

fn dependency_kind(value: &str) -> rusqlite::Result<DependencyKind> {
    match value {
        "finish_to_start" => Ok(DependencyKind::FinishToStart),
        "start_to_start" => Ok(DependencyKind::StartToStart),
        "finish_to_finish" => Ok(DependencyKind::FinishToFinish),
        "start_to_finish" => Ok(DependencyKind::StartToFinish),
        _ => conversion_error("invalid dependency kind"),
    }
}

fn json_value<T: serde::de::DeserializeOwned>(bytes: &[u8], column: usize) -> rusqlite::Result<T> {
    serde_json::from_slice(bytes).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(column, Type::Blob, Box::new(error))
    })
}

fn conversion_error<T>(message: &str) -> rusqlite::Result<T> {
    Err(rusqlite::Error::FromSqlConversionFailure(
        0,
        Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            message,
        )),
    ))
}

fn collect<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>>,
) -> Result<Vec<T>> {
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}
