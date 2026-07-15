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
        "SELECT id, agent_id, name, folder_path, created_at, updated_at \
         FROM workspaces WHERE agent_id = ?1 ORDER BY created_at ASC",
    )?;
    let rows = stmt.query_map([agent_id], |r| {
        Ok(WorkspaceRow {
            id: r.get(0)?,
            agent_id: r.get(1)?,
            name: r.get(2)?,
            folder_path: r.get(3)?,
            created_at: r.get(4)?,
            updated_at: r.get(5)?,
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
            "SELECT id, agent_id, name, folder_path, created_at, updated_at \
             FROM workspaces WHERE id = ?1",
            [id],
            |r| {
                Ok(WorkspaceRow {
                    id: r.get(0)?,
                    agent_id: r.get(1)?,
                    name: r.get(2)?,
                    folder_path: r.get(3)?,
                    created_at: r.get(4)?,
                    updated_at: r.get(5)?,
                })
            },
        )
        .optional()?;
    Ok(res)
}

/// 插入一个新工作区。
pub fn insert(conn: &Connection, w: &NewWorkspace) -> AppResult<String> {
    let now_str = now();
    conn.execute(
        "INSERT INTO workspaces (id, agent_id, name, folder_path, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![w.id, w.agent_id, w.name, w.folder_path, now_str, now_str],
    )?;
    Ok(w.id.clone())
}

/// 重命名工作区。
pub fn rename(conn: &Connection, id: &str, name: &str) -> AppResult<()> {
    conn.execute(
        "UPDATE workspaces SET name = ?1, updated_at = ?2 WHERE id = ?3",
        params![name, now(), id],
    )?;
    Ok(())
}

/// 删除工作区（会话的 workspace_id 置 NULL，会话本身保留）。
pub fn delete(conn: &Connection, id: &str) -> AppResult<()> {
    conn.execute("UPDATE sessions SET workspace_id = NULL WHERE workspace_id = ?1", [id])?;
    conn.execute("DELETE FROM workspaces WHERE id = ?1", [id])?;
    Ok(())
}
