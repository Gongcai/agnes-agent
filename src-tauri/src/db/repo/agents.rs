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
    pub created_at: String,
    pub updated_at: String,
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
}

pub fn list(conn: &Connection) -> AppResult<Vec<AgentRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, persona, scenario, system_prompt, greeting, \
         example_dialogue, model, tool_policy, avatar, tags, created_at, updated_at \
         FROM agents ORDER BY created_at",
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
            created_at: r.get(11)?,
            updated_at: r.get(12)?,
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
         example_dialogue, model, tool_policy, avatar, tags, created_at, updated_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
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
         avatar = ?9, tags = ?10, updated_at = ?11 WHERE id = ?12",
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
            &now,
            id,
        ),
    )?;
    Ok(())
}

pub fn update_model(conn: &Connection, id: &str, model: &str) -> AppResult<()> {
    let now = now();
    conn.execute(
        "UPDATE agents SET model = ?1, updated_at = ?2 WHERE id = ?3",
        (model, &now, id),
    )?;
    Ok(())
}

/// 删除角色卡：先删其会话（及关联消息）再删角色卡本体，避免 FK 约束报错
/// （sessions.agent_id REFERENCES agents(id) 无 ON DELETE CASCADE）。
pub fn delete(conn: &Connection, id: &str) -> AppResult<()> {
    // 关联消息先删（messages.session_id REFERENCES sessions(id)）
    conn.execute(
        "DELETE FROM messages WHERE session_id IN (SELECT id FROM sessions WHERE agent_id = ?1)",
        [id],
    )?;
    conn.execute("DELETE FROM sessions WHERE agent_id = ?1", [id])?;
    conn.execute("DELETE FROM agents WHERE id = ?1", [id])?;
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
