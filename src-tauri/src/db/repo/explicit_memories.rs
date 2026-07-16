use rusqlite::{params, Connection, Transaction};

use crate::error::{AppError, AppResult};

pub const USER_MD_KIND: &str = "user_md";
pub const MEMORY_MD_KIND: &str = "memory_md";

#[derive(Debug, Clone)]
pub struct ExplicitMemoryRow {
    pub id: String,
    pub agent_id: String,
    pub kind: String,
    pub content: String,
    pub created_at: String,
    pub updated_at: String,
    pub version: i64,
    pub deleted_at: Option<String>,
    pub origin_device_id: Option<String>,
}

fn now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    secs.to_string()
}

fn validate_kind(kind: &str) -> AppResult<()> {
    if matches!(kind, USER_MD_KIND | MEMORY_MD_KIND) {
        Ok(())
    } else {
        Err(AppError::Other(format!(
            "Unsupported explicit memory kind `{kind}`"
        )))
    }
}

fn row_from_sql(row: &rusqlite::Row<'_>) -> rusqlite::Result<ExplicitMemoryRow> {
    Ok(ExplicitMemoryRow {
        id: row.get(0)?,
        agent_id: row.get(1)?,
        kind: row.get(2)?,
        content: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
        version: row.get(6)?,
        deleted_at: row.get(7)?,
        origin_device_id: row.get(8)?,
    })
}

pub fn list(conn: &Connection, agent_id: &str) -> AppResult<Vec<ExplicitMemoryRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, agent_id, kind, content, created_at, updated_at, version, \
         deleted_at, origin_device_id FROM explicit_memories \
         WHERE agent_id = ?1 ORDER BY kind",
    )?;
    let rows = stmt.query_map([agent_id], row_from_sql)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn upsert(
    tx: &Transaction<'_>,
    id: &str,
    agent_id: &str,
    kind: &str,
    content: &str,
    timestamp: &str,
) -> AppResult<()> {
    validate_kind(kind)?;
    tx.execute(
        "INSERT INTO explicit_memories \
         (id, agent_id, kind, content, created_at, updated_at, version) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?5, 1) \
         ON CONFLICT(agent_id, kind) DO UPDATE SET \
           content = excluded.content, updated_at = excluded.updated_at, \
           version = explicit_memories.version + 1, deleted_at = NULL \
         WHERE explicit_memories.content IS NOT excluded.content \
            OR explicit_memories.deleted_at IS NOT NULL",
        params![id, agent_id, kind, content, timestamp],
    )?;
    Ok(())
}

pub fn save_pair(
    conn: &mut Connection,
    agent_id: &str,
    user_id: &str,
    user_md: &str,
    memory_id: &str,
    memory_md: &str,
) -> AppResult<()> {
    let timestamp = now();
    let tx = conn.transaction()?;
    upsert(&tx, user_id, agent_id, USER_MD_KIND, user_md, &timestamp)?;
    upsert(
        &tx,
        memory_id,
        agent_id,
        MEMORY_MD_KIND,
        memory_md,
        &timestamp,
    )?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_a_pair_atomically_and_only_versions_real_changes() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE agents (id TEXT PRIMARY KEY); \
             INSERT INTO agents (id) VALUES ('agent-1'); \
             CREATE TABLE explicit_memories ( \
               id TEXT PRIMARY KEY, agent_id TEXT NOT NULL REFERENCES agents(id), \
               kind TEXT NOT NULL CHECK(kind IN ('user_md', 'memory_md')), \
               content TEXT NOT NULL DEFAULT '', created_at TEXT NOT NULL, updated_at TEXT NOT NULL, \
               version INTEGER NOT NULL DEFAULT 1, deleted_at TEXT, origin_device_id TEXT, \
               UNIQUE(agent_id, kind));",
        )
        .unwrap();

        save_pair(
            &mut conn,
            "agent-1",
            "user-1",
            "User",
            "memory-1",
            "Memory",
        )
        .unwrap();
        save_pair(
            &mut conn,
            "agent-1",
            "unused-user-id",
            "User",
            "unused-memory-id",
            "Memory",
        )
        .unwrap();
        let rows = list(&conn, "agent-1").unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.version == 1));

        save_pair(
            &mut conn,
            "agent-1",
            "unused-user-id",
            "Updated user",
            "unused-memory-id",
            "Memory",
        )
        .unwrap();
        let rows = list(&conn, "agent-1").unwrap();
        assert_eq!(
            rows.iter()
                .find(|row| row.kind == USER_MD_KIND)
                .unwrap()
                .version,
            2
        );
        assert_eq!(
            rows.iter()
                .find(|row| row.kind == MEMORY_MD_KIND)
                .unwrap()
                .version,
            1
        );
    }
}
