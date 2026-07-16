use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::error::{AppError, AppResult};
use crate::sync::payload::SyncEntityType;

pub const USER_MD_KIND: &str = "user_md";
pub const MEMORY_MD_KIND: &str = "memory_md";

#[derive(Debug, Clone, Serialize)]
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

pub fn sync_entity_id(agent_id: &str, kind: &str) -> AppResult<String> {
    validate_kind(kind)?;
    let digest = Sha256::digest(format!("{agent_id}\0{kind}").as_bytes());
    Ok(format!("explicit_memory:{digest:x}"))
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
    let before = tx
        .query_row(
            "SELECT content, deleted_at FROM explicit_memories WHERE agent_id = ?1 AND kind = ?2",
            params![agent_id, kind],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()?;
    let device_id = super::sync::device_id(tx)?;
    tx.execute(
        "INSERT INTO explicit_memories \
         (id, agent_id, kind, content, created_at, updated_at, version, origin_device_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?5, 1, ?6) \
         ON CONFLICT(agent_id, kind) DO UPDATE SET \
           content = excluded.content, updated_at = excluded.updated_at, \
           version = explicit_memories.version + 1, deleted_at = NULL, \
           origin_device_id = excluded.origin_device_id \
         WHERE explicit_memories.content IS NOT excluded.content \
            OR explicit_memories.deleted_at IS NOT NULL",
        params![id, agent_id, kind, content, timestamp, device_id],
    )?;
    let changed = before
        .as_ref()
        .is_none_or(|(old_content, deleted_at)| old_content != content || deleted_at.is_some());
    if changed {
        let row = get_by_kind(tx, agent_id, kind)?.ok_or_else(|| {
            AppError::Other(format!("explicit memory `{agent_id}/{kind}` disappeared"))
        })?;
        enqueue_current(tx, &row)?;
    }
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
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
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

fn get_by_kind(
    conn: &Connection,
    agent_id: &str,
    kind: &str,
) -> AppResult<Option<ExplicitMemoryRow>> {
    conn.query_row(
        "SELECT id, agent_id, kind, content, created_at, updated_at, version, \
         deleted_at, origin_device_id FROM explicit_memories \
         WHERE agent_id = ?1 AND kind = ?2",
        params![agent_id, kind],
        row_from_sql,
    )
    .optional()
    .map_err(Into::into)
}

fn enqueue_current(conn: &Connection, row: &ExplicitMemoryRow) -> AppResult<()> {
    let mut source = serde_json::to_value(row)?;
    source
        .as_object_mut()
        .ok_or_else(|| AppError::Other("explicit memory sync source is not an object".into()))?
        .insert(
            "id".into(),
            sync_entity_id(&row.agent_id, &row.kind)?.into(),
        );
    super::sync::enqueue_projection(
        conn,
        SyncEntityType::ExplicitMemory,
        &sync_entity_id(&row.agent_id, &row.kind)?,
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
    fn saves_a_pair_atomically_and_only_versions_real_changes() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::db::schema::SCHEMA).unwrap();
        conn.execute(
            "INSERT INTO agents (id, name) VALUES ('agent-1', 'Agent')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sync_runtime_state (singleton, device_id) \
             VALUES (1, '00000000-0000-4000-8000-000000000001')",
            [],
        )
        .unwrap();

        save_pair(&mut conn, "agent-1", "user-1", "User", "memory-1", "Memory").unwrap();
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
        let outbox_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_outbox WHERE entity_type = 'explicit_memory'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(outbox_count, 3);
    }
}
