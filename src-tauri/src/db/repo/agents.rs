use rusqlite::Connection;

use crate::error::AppResult;

#[derive(Debug, Clone)]
pub struct AgentRow {
    pub id: String,
    pub name: String,
    pub persona: String,
    pub scenario: String,
    pub system_prompt: String,
    pub greeting: String,
    pub example_dialogue: String,
    pub model: String,
    pub tool_policy: String,
    pub avatar: String,
    pub tags: String,
    pub thinking_mode: String,
    pub thinking_budget: i64,
    pub created_at: String,
    pub updated_at: String,
    pub version: i64,
    pub deleted_at: Option<String>,
    pub origin_device_id: Option<String>,
}

pub struct NewAgent {
    pub id: String,
    pub name: String,
    pub persona: String,
    pub scenario: String,
    pub system_prompt: String,
    pub greeting: String,
    pub example_dialogue: String,
    pub model: String,
    pub tool_policy: String,
    pub avatar: String,
    pub tags: String,
    pub thinking_mode: String,
    pub thinking_budget: i64,
}

/// 角色卡可编辑字段（不含 id）。
pub struct AgentUpdate {
    pub name: String,
    pub persona: String,
    pub scenario: String,
    pub system_prompt: String,
    pub greeting: String,
    pub example_dialogue: String,
    pub model: String,
    pub tool_policy: String,
    pub avatar: String,
    pub tags: String,
    pub thinking_mode: String,
    pub thinking_budget: i64,
}

pub fn list(conn: &Connection) -> AppResult<Vec<AgentRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, persona, scenario, system_prompt, greeting, \
         example_dialogue, model, tool_policy, avatar, tags, thinking_mode, thinking_budget, \
         created_at, updated_at, version, deleted_at, origin_device_id \
         FROM agents WHERE deleted_at IS NULL ORDER BY created_at",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(AgentRow {
            id: r.get(0)?,
            name: r.get(1)?,
            persona: r.get(2)?,
            scenario: r.get(3)?,
            system_prompt: r.get(4)?,
            greeting: r.get(5)?,
            example_dialogue: r.get(6)?,
            model: r.get(7)?,
            tool_policy: r.get(8)?,
            avatar: r.get(9)?,
            tags: r.get(10)?,
            thinking_mode: r.get(11)?,
            thinking_budget: r.get(12)?,
            created_at: r.get(13)?,
            updated_at: r.get(14)?,
            version: r.get(15)?,
            deleted_at: r.get(16)?,
            origin_device_id: r.get(17)?,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

pub fn insert(conn: &Connection, a: &NewAgent) -> AppResult<String> {
    let now = now();
    conn.execute(
        "INSERT INTO agents (id, name, persona, scenario, system_prompt, greeting, \
         example_dialogue, model, tool_policy, avatar, tags, thinking_mode, thinking_budget, \
         created_at, updated_at, version) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,1)",
        (
            &a.id,
            &a.name,
            &a.persona,
            &a.scenario,
            &a.system_prompt,
            &a.greeting,
            &a.example_dialogue,
            &a.model,
            &a.tool_policy,
            &a.avatar,
            &a.tags,
            &a.thinking_mode,
            &a.thinking_budget,
            &now,
            &now,
        ),
    )?;
    Ok(a.id.clone())
}

pub fn update(conn: &Connection, id: &str, c: &AgentUpdate) -> AppResult<()> {
    let now = now();
    conn.execute(
        "UPDATE agents SET name = ?1, persona = ?2, scenario = ?3, system_prompt = ?4, \
         greeting = ?5, example_dialogue = ?6, model = ?7, tool_policy = ?8, \
         avatar = ?9, tags = ?10, thinking_mode = ?11, thinking_budget = ?12, \
         updated_at = ?13, version = version + 1 WHERE id = ?14 AND deleted_at IS NULL",
        (
            &c.name,
            &c.persona,
            &c.scenario,
            &c.system_prompt,
            &c.greeting,
            &c.example_dialogue,
            &c.model,
            &c.tool_policy,
            &c.avatar,
            &c.tags,
            &c.thinking_mode,
            &c.thinking_budget,
            &now,
            id,
        ),
    )?;
    Ok(())
}

pub fn update_model(conn: &Connection, id: &str, model: &str) -> AppResult<()> {
    let now = now();
    conn.execute(
        "UPDATE agents SET model = ?1, updated_at = ?2, version = version + 1 \
         WHERE id = ?3 AND deleted_at IS NULL",
        (model, &now, id),
    )?;
    Ok(())
}

/// Soft-delete an agent while retaining dependent rows for sync and later compaction.
pub fn delete(conn: &Connection, id: &str) -> AppResult<()> {
    let now = now();
    conn.execute(
        "UPDATE agents SET deleted_at = ?1, updated_at = ?1, version = version + 1 \
         WHERE id = ?2 AND deleted_at IS NULL",
        (&now, id),
    )?;
    Ok(())
}

pub fn now() -> String {
    chrono_like_now()
}

/// 轻量时间戳（避免引入 chrono 依赖；格式满足排序即可）。
fn chrono_like_now() -> String {
    // 用 std 系统时间拼 ISO-ish 字符串
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_agent() -> NewAgent {
        NewAgent {
            id: "agent-1".into(),
            name: "Agent".into(),
            persona: String::new(),
            scenario: String::new(),
            system_prompt: String::new(),
            greeting: String::new(),
            example_dialogue: String::new(),
            model: String::new(),
            tool_policy: "{}".into(),
            avatar: String::new(),
            tags: String::new(),
            thinking_mode: "off".into(),
            thinking_budget: 0,
        }
    }

    #[test]
    fn agent_updates_and_deletes_advance_the_tombstone_version() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::db::schema::SCHEMA).unwrap();
        insert(&conn, &new_agent()).unwrap();
        update_model(&conn, "agent-1", "provider/model").unwrap();
        delete(&conn, "agent-1").unwrap();

        assert!(list(&conn).unwrap().is_empty());
        let (version, deleted_at): (i64, Option<String>) = conn
            .query_row(
                "SELECT version, deleted_at FROM agents WHERE id = 'agent-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(version, 3);
        assert!(deleted_at.is_some());
    }
}
