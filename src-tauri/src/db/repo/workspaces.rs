//! workspaces repo - 工作区（绑定文件夹）的 CRUD。
use rusqlite::{params, Connection, OptionalExtension};

use crate::error::AppResult;

#[derive(Debug, Clone)]
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
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO workspaces (id, agent_id, name, created_at, updated_at, version) \
         VALUES (?1, ?2, ?3, ?4, ?4, 1)",
        params![w.id, w.agent_id, w.name, now_str],
    )?;
    tx.execute(
        "INSERT INTO workspace_bindings \
         (workspace_id, folder_path, created_at, updated_at, last_validated_at) \
         VALUES (?1, ?2, ?3, ?3, ?3)",
        params![w.id, w.folder_path, now_str],
    )?;
    tx.commit()?;
    Ok(w.id.clone())
}

/// 重命名工作区。
pub fn rename(conn: &Connection, id: &str, name: &str) -> AppResult<()> {
    conn.execute(
        "UPDATE workspaces SET name = ?1, updated_at = ?2, version = version + 1 \
         WHERE id = ?3 AND deleted_at IS NULL",
        params![name, now(), id],
    )?;
    Ok(())
}

/// Soft-delete the logical workspace and remove only this device's path binding.
pub fn delete(conn: &mut Connection, id: &str) -> AppResult<()> {
    let now_str = now();
    let tx = conn.transaction()?;
    tx.execute(
        "UPDATE sessions SET workspace_id = NULL, updated_at = ?1, version = version + 1 \
         WHERE workspace_id = ?2 AND deleted_at IS NULL",
        params![now_str, id],
    )?;
    tx.execute(
        "UPDATE workspaces SET deleted_at = ?1, updated_at = ?1, version = version + 1 \
         WHERE id = ?2 AND deleted_at IS NULL",
        params![now_str, id],
    )?;
    tx.execute(
        "DELETE FROM workspace_bindings WHERE workspace_id = ?1",
        [id],
    )?;
    tx.commit()?;
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
    }
}
