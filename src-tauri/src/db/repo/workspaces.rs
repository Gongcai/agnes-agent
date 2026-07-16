//! workspaces repo - 工作区（绑定文件夹）的 CRUD。
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;

use crate::error::{AppError, AppResult};
use crate::sync::payload::SyncEntityType;

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceRow {
    pub id: String,
    pub agent_id: String,
    pub name: String,
    pub folder_path: String,
    pub created_at: String,
    pub updated_at: String,
    pub version: i64,
    pub deleted_at: Option<String>,
    pub origin_device_id: Option<String>,
}

pub struct NewWorkspace {
    pub id: String,
    pub agent_id: String,
    pub name: String,
    pub folder_path: String,
}

fn now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

/// 列出某 agent 的所有工作区，按创建时间升序。
pub fn list(conn: &Connection, agent_id: &str) -> AppResult<Vec<WorkspaceRow>> {
    let mut stmt = conn.prepare(
        "SELECT w.id, w.agent_id, w.name, COALESCE(b.folder_path, ''), \
                w.created_at, w.updated_at, w.version, w.deleted_at, w.origin_device_id \
         FROM workspaces w LEFT JOIN workspace_bindings b ON b.workspace_id = w.id \
         WHERE w.agent_id = ?1 AND w.deleted_at IS NULL ORDER BY w.created_at ASC",
    )?;
    let rows = stmt.query_map([agent_id], |r| {
        Ok(WorkspaceRow {
            id: r.get(0)?,
            agent_id: r.get(1)?,
            name: r.get(2)?,
            folder_path: r.get(3)?,
            created_at: r.get(4)?,
            updated_at: r.get(5)?,
            version: r.get(6)?,
            deleted_at: r.get(7)?,
            origin_device_id: r.get(8)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// 按 id 取单个工作区。
pub fn get(conn: &Connection, id: &str) -> AppResult<Option<WorkspaceRow>> {
    let res = conn
        .query_row(
            "SELECT w.id, w.agent_id, w.name, COALESCE(b.folder_path, ''), \
                    w.created_at, w.updated_at, w.version, w.deleted_at, w.origin_device_id \
             FROM workspaces w LEFT JOIN workspace_bindings b ON b.workspace_id = w.id \
             WHERE w.id = ?1 AND w.deleted_at IS NULL",
            [id],
            |r| {
                Ok(WorkspaceRow {
                    id: r.get(0)?,
                    agent_id: r.get(1)?,
                    name: r.get(2)?,
                    folder_path: r.get(3)?,
                    created_at: r.get(4)?,
                    updated_at: r.get(5)?,
                    version: r.get(6)?,
                    deleted_at: r.get(7)?,
                    origin_device_id: r.get(8)?,
                })
            },
        )
        .optional()?;
    Ok(res)
}

/// 插入一个新工作区。
pub fn insert(conn: &mut Connection, w: &NewWorkspace) -> AppResult<String> {
    let now_str = now();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let device_id = super::sync::device_id(&tx)?;
    tx.execute(
        "INSERT INTO workspaces (id, agent_id, name, created_at, updated_at, version, origin_device_id) \
         VALUES (?1, ?2, ?3, ?4, ?4, 1, ?5)",
        params![w.id, w.agent_id, w.name, now_str, device_id],
    )?;
    tx.execute(
        "INSERT INTO workspace_bindings \
         (workspace_id, folder_path, created_at, updated_at, last_validated_at) \
         VALUES (?1, ?2, ?3, ?3, ?3)",
        params![w.id, w.folder_path, now_str],
    )?;
    enqueue_current(&tx, &w.id)?;
    tx.commit()?;
    Ok(w.id.clone())
}

/// 重命名工作区。
pub fn rename(conn: &mut Connection, id: &str, name: &str) -> AppResult<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let device_id = super::sync::device_id(&tx)?;
    let changed = tx.execute(
        "UPDATE workspaces SET name = ?1, updated_at = ?2, version = version + 1, \
         origin_device_id = ?3 WHERE id = ?4 AND deleted_at IS NULL",
        params![name, now(), device_id, id],
    )?;
    if changed > 0 {
        enqueue_current(&tx, id)?;
    }
    tx.commit()?;
    Ok(())
}

/// Soft-delete the logical workspace and remove only this device's path binding.
pub fn delete(conn: &mut Connection, id: &str) -> AppResult<()> {
    let now_str = now();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let device_id = super::sync::device_id(&tx)?;
    let session_ids = {
        let mut statement =
            tx.prepare("SELECT id FROM sessions WHERE workspace_id = ?1 AND deleted_at IS NULL")?;
        let rows = statement
            .query_map([id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    tx.execute(
        "UPDATE sessions SET workspace_id = NULL, updated_at = ?1, version = version + 1, \
         origin_device_id = ?2 WHERE workspace_id = ?3 AND deleted_at IS NULL",
        params![now_str, device_id, id],
    )?;
    for session_id in session_ids {
        super::sessions::enqueue_current(&tx, &session_id)?;
    }
    let changed = tx.execute(
        "UPDATE workspaces SET deleted_at = ?1, updated_at = ?1, version = version + 1, \
         origin_device_id = ?2 WHERE id = ?3 AND deleted_at IS NULL",
        params![now_str, device_id, id],
    )?;
    tx.execute(
        "DELETE FROM workspace_bindings WHERE workspace_id = ?1",
        [id],
    )?;
    if changed > 0 {
        enqueue_current(&tx, id)?;
    }
    tx.commit()?;
    Ok(())
}

fn get_any(conn: &Connection, id: &str) -> AppResult<Option<WorkspaceRow>> {
    conn.query_row(
        "SELECT w.id, w.agent_id, w.name, COALESCE(b.folder_path, ''), \
                w.created_at, w.updated_at, w.version, w.deleted_at, w.origin_device_id \
         FROM workspaces w LEFT JOIN workspace_bindings b ON b.workspace_id = w.id \
         WHERE w.id = ?1",
        [id],
        |row| {
            Ok(WorkspaceRow {
                id: row.get(0)?,
                agent_id: row.get(1)?,
                name: row.get(2)?,
                folder_path: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
                version: row.get(6)?,
                deleted_at: row.get(7)?,
                origin_device_id: row.get(8)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

fn enqueue_current(conn: &Connection, id: &str) -> AppResult<()> {
    let row = get_any(conn, id)?.ok_or_else(|| {
        AppError::Other(format!("workspace `{id}` disappeared during sync enqueue"))
    })?;
    let source = serde_json::to_value(&row)?;
    super::sync::enqueue_projection(
        conn,
        SyncEntityType::Workspace,
        &row.id,
        row.version,
        row.deleted_at.is_some(),
        &source,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_delete_removes_the_local_binding_and_keeps_a_tombstone() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::db::schema::SCHEMA).unwrap();
        conn.execute(
            "INSERT INTO agents (id, name) VALUES ('agent-1', 'Agent')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sync_runtime_state (singleton, device_id) VALUES (1, '12345678-device')",
            [],
        )
        .unwrap();
        insert(
            &mut conn,
            &NewWorkspace {
                id: "workspace-1".into(),
                agent_id: "agent-1".into(),
                name: "Project".into(),
                folder_path: "/tmp/project".into(),
            },
        )
        .unwrap();
        delete(&mut conn, "workspace-1").unwrap();

        assert!(get(&conn, "workspace-1").unwrap().is_none());
        let binding_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM workspace_bindings WHERE workspace_id = 'workspace-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(binding_count, 0);
        let (version, deleted_at): (i64, Option<String>) = conn
            .query_row(
                "SELECT version, deleted_at FROM workspaces WHERE id = 'workspace-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(version, 2);
        assert!(deleted_at.is_some());
        let operations: Vec<String> = conn
            .prepare(
                "SELECT operation FROM sync_outbox WHERE entity_type = 'workspace' ORDER BY rowid",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(operations, vec!["upsert", "delete"]);
    }
}
