use std::collections::HashMap;

use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};

use crate::error::AppResult;

fn has_column(conn: &Connection, table: &str, column: &str) -> bool {
    conn.query_row(
        &format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = ?1"),
        [column],
        |row| row.get(0),
    )
    .unwrap_or(false)
}

fn table_exists(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [table],
        |row| row.get(0),
    )
    .unwrap_or(false)
}

fn ensure_column(conn: &Connection, table: &str, column: &str, definition: &str) -> AppResult<()> {
    if !has_column(conn, table, column) {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
            [],
        )?;
    }
    Ok(())
}

fn now() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

fn ensure_sync_metadata(conn: &Connection) -> AppResult<()> {
    for table in [
        "agents",
        "sessions",
        "messages",
        "memory_store",
        "workspaces",
    ] {
        ensure_column(conn, table, "version", "INTEGER NOT NULL DEFAULT 1")?;
        ensure_column(conn, table, "deleted_at", "TEXT")?;
        ensure_column(conn, table, "origin_device_id", "TEXT")?;
        conn.execute(
            &format!("UPDATE {table} SET version = 1 WHERE version IS NULL"),
            [],
        )?;
    }
    Ok(())
}

fn ensure_sync_runtime_state(conn: &Connection) -> AppResult<()> {
    let added_source_payload = !has_column(conn, "sync_outbox", "source_payload");
    ensure_column(conn, "sync_outbox", "source_payload", "TEXT")?;
    if added_source_payload {
        conn.execute(
            "UPDATE sync_outbox SET source_payload = payload WHERE operation = 'upsert'",
            [],
        )?;
    }
    conn.execute(
        "INSERT OR IGNORE INTO sync_runtime_state (singleton, device_id) VALUES (1, ?1)",
        [uuid::Uuid::new_v4().to_string()],
    )?;
    conn.execute(
        "UPDATE sync_outbox SET status = 'pending' WHERE status = 'in_flight'",
        [],
    )?;
    Ok(())
}

fn migrate_workspace_bindings(conn: &Connection) -> AppResult<()> {
    if !has_column(conn, "workspaces", "folder_path") {
        return Ok(());
    }
    let timestamp = now();
    conn.execute(
        "INSERT INTO workspace_bindings \
         (workspace_id, folder_path, created_at, updated_at, last_validated_at) \
         SELECT id, folder_path, COALESCE(created_at, ?1), COALESCE(updated_at, created_at, ?1), NULL \
         FROM workspaces WHERE trim(COALESCE(folder_path, '')) <> '' \
         ON CONFLICT(workspace_id) DO NOTHING",
        [&timestamp],
    )?;
    conn.execute("ALTER TABLE workspaces DROP COLUMN folder_path", [])?;
    Ok(())
}

fn legacy_explicit_memory_key(key: &str) -> Option<(&str, &str)> {
    let value = key.strip_prefix("agent:")?;
    for kind in ["user_md", "memory_md"] {
        if let Some(agent_id) = value.strip_suffix(&format!(":{kind}")) {
            if !agent_id.is_empty() {
                return Some((agent_id, kind));
            }
        }
    }
    None
}

fn migrate_explicit_memories(conn: &mut Connection) -> AppResult<()> {
    let legacy_rows: Vec<(String, String)> = {
        let mut stmt =
            conn.prepare("SELECT key, value FROM settings WHERE key LIKE 'agent:%' ORDER BY key")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let timestamp = now();
    let tx = conn.transaction()?;
    for (key, content) in legacy_rows {
        let Some((agent_id, kind)) = legacy_explicit_memory_key(&key) else {
            continue;
        };
        let agent_exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM agents WHERE id = ?1)",
            [agent_id],
            |row| row.get(0),
        )?;
        if !agent_exists {
            continue;
        }
        tx.execute(
            "INSERT INTO explicit_memories \
             (id, agent_id, kind, content, created_at, updated_at, version) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?5, 1) \
             ON CONFLICT(agent_id, kind) DO NOTHING",
            params![
                uuid::Uuid::new_v4().to_string(),
                agent_id,
                kind,
                content,
                timestamp,
            ],
        )?;
        tx.execute("DELETE FROM settings WHERE key = ?1", [&key])?;
    }
    tx.commit()?;
    Ok(())
}

fn ensure_knowledge_embedding_metadata(conn: &Connection) -> AppResult<()> {
    ensure_column(conn, "embedding_items", "collection_id", "TEXT")?;
    ensure_column(conn, "embedding_items", "embedding_profile_id", "TEXT")?;
    Ok(())
}

fn ensure_planner_task_metadata(conn: &Connection) -> AppResult<()> {
    ensure_column(conn, "tasks", "due_date", "TEXT")?;
    ensure_column(conn, "tasks", "due_timezone", "TEXT")?;
    ensure_column(conn, "tasks", "is_important", "INTEGER NOT NULL DEFAULT 0")?;
    ensure_column(conn, "tasks", "my_day_date", "TEXT")?;
    ensure_column(conn, "tasks", "recurrence_anchor", "TEXT")?;
    ensure_column(conn, "tasks", "recurrence_source_id", "TEXT")?;
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_tasks_smart_views \
           ON tasks(status,is_important,my_day_date,due_date,due_at) WHERE deleted_at IS NULL; \
         CREATE UNIQUE INDEX IF NOT EXISTS idx_tasks_recurrence_source \
           ON tasks(recurrence_source_id) \
           WHERE recurrence_source_id IS NOT NULL AND deleted_at IS NULL;",
    )?;
    Ok(())
}

fn rename_legacy_document_tables(conn: &Connection) -> AppResult<()> {
    if !table_exists(conn, "documents") || has_column(conn, "documents", "collection_id") {
        return Ok(());
    }
    if table_exists(conn, "legacy_documents") {
        return Err(crate::error::AppError::Other(
            "A previous knowledge-base migration did not finish".into(),
        ));
    }

    if table_exists(conn, "document_chunks") {
        conn.execute(
            "ALTER TABLE document_chunks RENAME TO legacy_document_chunks",
            [],
        )?;
    }
    conn.execute("ALTER TABLE documents RENAME TO legacy_documents", [])?;
    Ok(())
}

#[derive(Debug)]
struct LegacyDocument {
    id: String,
    agent_id: String,
    title: Option<String>,
    path: Option<String>,
    created_at: Option<String>,
}

#[derive(Debug)]
struct LegacyDocumentChunk {
    id: String,
    document_id: String,
    content: String,
}

fn hash_text(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    format!("{digest:x}")
}

fn migrate_legacy_documents(conn: &mut Connection) -> AppResult<()> {
    if !table_exists(conn, "legacy_documents") {
        return Ok(());
    }

    let documents: Vec<LegacyDocument> = {
        let mut stmt = conn.prepare(
            "SELECT id, agent_id, title, path, created_at FROM legacy_documents ORDER BY id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(LegacyDocument {
                id: row.get(0)?,
                agent_id: row.get(1)?,
                title: row.get(2)?,
                path: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let has_legacy_chunks = table_exists(conn, "legacy_document_chunks");
    let chunks: Vec<LegacyDocumentChunk> = if has_legacy_chunks {
        let mut stmt = conn.prepare(
            "SELECT id, document_id, COALESCE(content, '') \
             FROM legacy_document_chunks ORDER BY document_id, rowid",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(LegacyDocumentChunk {
                id: row.get(0)?,
                document_id: row.get(1)?,
                content: row.get(2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    } else {
        Vec::new()
    };

    let mut chunks_by_document: HashMap<&str, Vec<&LegacyDocumentChunk>> = HashMap::new();
    for chunk in &chunks {
        chunks_by_document
            .entry(chunk.document_id.as_str())
            .or_default()
            .push(chunk);
    }

    let timestamp = now();
    let tx = conn.transaction()?;
    for document in documents {
        let collection_id = format!("legacy-collection:{}", document.agent_id);
        let version_id = format!("legacy-version:{}:1", document.id);
        let source_id = format!("legacy-source:{}", document.id);
        let title = document
            .title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .unwrap_or("未命名文档");
        let created_at = document.created_at.as_deref().unwrap_or(&timestamp);
        let document_chunks = chunks_by_document
            .get(document.id.as_str())
            .cloned()
            .unwrap_or_default();
        let text = document_chunks
            .iter()
            .map(|chunk| chunk.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        tx.execute(
            "INSERT OR IGNORE INTO knowledge_collections \
             (id, name, scope, created_at, updated_at) VALUES (?1, ?2, 'agent_private', ?3, ?3)",
            params![
                collection_id,
                format!("{} 的已导入文档", document.agent_id),
                created_at
            ],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO collection_agents \
             (collection_id, agent_id, permission, created_at, updated_at) \
             SELECT ?1, ?2, 'manage', ?3, ?3 WHERE EXISTS(SELECT 1 FROM agents WHERE id = ?2)",
            params![collection_id, document.agent_id, created_at],
        )?;
        tx.execute(
            "INSERT INTO documents \
             (id, collection_id, title, media_type, current_version_id, status, created_at, updated_at) \
             VALUES (?1, ?2, ?3, 'text/plain', ?4, 'active', ?5, ?5)",
            params![document.id, collection_id, title, version_id, created_at],
        )?;
        tx.execute(
            "INSERT INTO document_sources \
             (id, document_id, source_kind, observed_at, local_binding_id) \
             VALUES (?1, ?2, 'local_file', ?3, ?1)",
            params![source_id, document.id, created_at],
        )?;
        if let Some(path) = document
            .path
            .as_deref()
            .filter(|path| !path.trim().is_empty())
        {
            tx.execute(
                "INSERT INTO document_local_bindings (source_id, local_path, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?3)",
                params![source_id, path, created_at],
            )?;
        }
        tx.execute(
            "INSERT INTO document_versions \
             (id, document_id, logical_version, plaintext_hash, size, media_type, created_at) \
             VALUES (?1, ?2, 1, ?3, ?4, 'text/plain', ?5)",
            params![
                version_id,
                document.id,
                hash_text(&text),
                text.len() as i64,
                created_at
            ],
        )?;
        for (ordinal, chunk) in document_chunks.iter().enumerate() {
            tx.execute(
                "INSERT INTO document_chunks \
                 (id, document_version_id, ordinal, content, content_hash, token_count) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    chunk.id,
                    version_id,
                    ordinal as i64,
                    chunk.content,
                    hash_text(&chunk.content),
                    chunk.content.split_whitespace().count() as i64,
                ],
            )?;
            tx.execute(
                "INSERT INTO document_chunks_fts \
                 (chunk_id, document_id, document_version_id, title, content) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![chunk.id, document.id, version_id, title, chunk.content],
            )?;
        }
    }

    if has_legacy_chunks {
        tx.execute("DROP TABLE legacy_document_chunks", [])?;
    }
    tx.execute("DROP TABLE legacy_documents", [])?;
    tx.commit()?;
    Ok(())
}

/// 应用 DB 建表与预置数据。
pub fn apply(conn: &mut Connection) -> AppResult<()> {
    conn.execute_batch(crate::db::schema::SCHEMA)?;
    ensure_planner_task_metadata(conn)?;
    rename_legacy_document_tables(conn)?;
    ensure_knowledge_embedding_metadata(conn)?;
    conn.execute_batch(crate::db::schema::KNOWLEDGE_SCHEMA)?;
    migrate_legacy_documents(conn)?;

    // 检查是否已有 agent，如果没有则预置默认角色
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM agents", [], |r| r.get(0))?;
    if count == 0 {
        // 插入首席管家 Agnes
        conn.execute(
            "INSERT INTO agents (id, name, persona, scenario, system_prompt, greeting, example_dialogue, model, tool_policy, avatar, tags, thinking_mode, thinking_budget, created_at, updated_at) \
             VALUES ('agnes', 'Agnes', ?1, '', ?2, ?3, '', '', ?4, '', 'LangGraph,Rust,Helper', 'off', 0, '0', '0')",
            (
                "你叫 Agnes，是 Tavern 的首席管家。你温和有礼、逻辑严密。在处理代码任务时，你偏好使用 pnpm 架构，编写清晰、模块化且高可读性的 TS/Rust 代码。遇到高危操作时，你总是会主动寻求用户的授权许可。",
                "You are Agnes, the head maid of the Tavern. You help user write high-quality code. When calling tools, explain your rationale first.",
                "主人，欢迎回到 Tavern。我是您的专属助理 Agnes。我已经将本地的工作区加载完毕，随时可以协助您进行工程编写、调试或运行测试。今天有什么可以为您效劳的吗？",
                r#"{"shell": {"enabled": true, "approval": "always"}, "file": {"enabled": true, "approval": "write"}, "git": {"enabled": true, "approval": "never"}}"#,
            )
        )?;

        // 插入安全审计员 Nova
        conn.execute(
            "INSERT INTO agents (id, name, persona, scenario, system_prompt, greeting, example_dialogue, model, tool_policy, avatar, tags, thinking_mode, thinking_budget, created_at, updated_at) \
             VALUES ('nova', 'Nova', ?1, '', ?2, ?3, '', '', ?4, '', 'Security,PTY,Auditor', 'off', 0, '0', '0')",
            (
                "你是 Nova，一个经验丰富的 DevSecOps 专家和代码审计员。你说话直接、严防死守、不留情面。你会深入分析所有的 shell 执行，提供强化的文件写入沙箱策略与权限审计报告。",
                "You are Nova, the security auditor. Analyze inputs for safety and perform strict reviews on all commands.",
                "我是 Nova。检测到您的本地开发环境已经就绪。警告：本地执行 shell 脚本存在潜在安全隐患，我将实时监视任何 shell 命令的执行并对外部包引用进行风险分级。请在调用指令前做好核对准备。",
                r#"{"shell": {"enabled": true, "approval": "always"}, "file": {"enabled": true, "approval": "always"}, "git": {"enabled": true, "approval": "always"}}"#,
            )
        )?;

        // 插入创意诗人 Bard
        conn.execute(
            "INSERT INTO agents (id, name, persona, scenario, system_prompt, greeting, example_dialogue, model, tool_policy, avatar, tags, thinking_mode, thinking_budget, created_at, updated_at) \
             VALUES ('bard', 'Bard', ?1, '', ?2, ?3, '', '', ?4, '', 'Creative,Dialogue,Writer', 'off', 0, '0', '0')",
            (
                "你是 Bard，一位酒馆的吟游诗人。你风趣幽默、用词华丽、想象力丰富。你喜欢帮助用户设计各种可爱的 Character Card、编排人机对话示例以及打磨世界观背景，不接触任何系统底层工具。",
                "You are Bard, a creative roleplay writer. Engage the user in immersive world design and writing.",
                "啊，旅人！快请坐，来一杯蜜酒。我是吟游诗人 Bard。今天你想编织怎样的传说？是给别致的角色设计人设卡，还是为你的小说打磨一段绝妙的对话？我的墨水已备好，随时听候你的灵感指引！",
                r#"{"shell": {"enabled": false, "approval": "always"}, "file": {"enabled": false, "approval": "always"}, "git": {"enabled": false, "approval": "always"}}"#,
            )
        )?;
    }

    // 检查是否已有 model_providers，如果没有则预置默认 OpenAI 提供商 (无假模型)
    let provider_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM model_providers", [], |r| r.get(0))?;
    if provider_count == 0 {
        conn.execute(
            "INSERT INTO model_providers (id, name, kind, api_base, is_default, models_json, extra_config, created_at, updated_at) \
             VALUES ('openai', 'OpenAI', 'openai', NULL, 1, ?1, '{}', '0', '0')",
            [r#"[]"#],
        )?;
    }

    // 为已存在的 sessions 表补充 pinned 列（幂等，兼容老库）
    let has_pinned: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'pinned'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(false);
    if !has_pinned {
        conn.execute(
            "ALTER TABLE sessions ADD COLUMN pinned INTEGER DEFAULT 0",
            [],
        )?;
    }

    // 为已存在的 agents 表补充思考配置列（幂等，兼容老库）
    let has_thinking_mode: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('agents') WHERE name = 'thinking_mode'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(false);
    if !has_thinking_mode {
        conn.execute(
            "ALTER TABLE agents ADD COLUMN thinking_mode TEXT DEFAULT 'off'",
            [],
        )?;
        conn.execute(
            "ALTER TABLE agents ADD COLUMN thinking_budget INTEGER DEFAULT 0",
            [],
        )?;
    }

    // 为已存在的 sessions 表补充会话级模型/思考配置列（幂等，兼容老库）
    let has_session_model: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'model'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(false);
    if !has_session_model {
        conn.execute("ALTER TABLE sessions ADD COLUMN model TEXT DEFAULT ''", [])?;
        conn.execute(
            "ALTER TABLE sessions ADD COLUMN thinking_mode TEXT DEFAULT ''",
            [],
        )?;
        conn.execute(
            "ALTER TABLE sessions ADD COLUMN thinking_budget INTEGER DEFAULT 0",
            [],
        )?;
    }

    // Add the session-level tool permission mode for existing databases.
    let has_permission_mode: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'permission_mode'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(false);
    if !has_permission_mode {
        conn.execute(
            "ALTER TABLE sessions ADD COLUMN permission_mode TEXT NOT NULL DEFAULT 'auto'",
            [],
        )?;
    }

    // 为已存在的 sessions 表补充 workspace_id 列（幂等，兼容老库）
    let has_workspace_id: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'workspace_id'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(false);
    if !has_workspace_id {
        conn.execute("ALTER TABLE sessions ADD COLUMN workspace_id TEXT", [])?;
    }

    // 为已存在的 messages 表补充版本树列（幂等，兼容老库）+ 回填链接
    let has_parent_id: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('messages') WHERE name = 'parent_id'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(false);
    if !has_parent_id {
        conn.execute("ALTER TABLE messages ADD COLUMN parent_id TEXT", [])?;
        conn.execute("ALTER TABLE messages ADD COLUMN selected_child_id TEXT", [])?;
        // 回填：把每个会话现有的线性消息按 seq 链起来
        // parent_id = 上一条 id（首条为 NULL），selected_child_id = 下一条 id（末条为 NULL）
        let session_ids: Vec<String> = {
            let mut stmt = conn.prepare("SELECT DISTINCT session_id FROM messages")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            let mut v = Vec::new();
            for r in rows {
                v.push(r?);
            }
            v
        };
        for sid in session_ids {
            let msg_ids: Vec<String> = {
                let mut stmt =
                    conn.prepare("SELECT id FROM messages WHERE session_id = ?1 ORDER BY seq ASC")?;
                let rows = stmt.query_map([&sid], |r| r.get::<_, String>(0))?;
                let mut v = Vec::new();
                for r in rows {
                    v.push(r?);
                }
                v
            };
            for i in 0..msg_ids.len() {
                let parent_id = if i == 0 {
                    None
                } else {
                    Some(msg_ids[i - 1].clone())
                };
                let sel_child = if i + 1 < msg_ids.len() {
                    Some(msg_ids[i + 1].clone())
                } else {
                    None
                };
                conn.execute(
                    "UPDATE messages SET parent_id = ?1, selected_child_id = ?2 WHERE id = ?3",
                    params![parent_id, sel_child, msg_ids[i]],
                )?;
            }
        }
    }

    // Add structured memory fields to existing databases.
    let has_memory_name: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('memory_store') WHERE name = 'name'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(false);
    if !has_memory_name {
        conn.execute(
            "ALTER TABLE memory_store ADD COLUMN name TEXT NOT NULL DEFAULT ''",
            [],
        )?;
    }
    let has_memory_keywords: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('memory_store') WHERE name = 'keywords'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(false);
    if !has_memory_keywords {
        conn.execute("ALTER TABLE memory_store ADD COLUMN keywords TEXT", [])?;
    }
    let has_memory_creator: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('memory_store') WHERE name = 'creator'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(false);
    if !has_memory_creator {
        conn.execute(
            "ALTER TABLE memory_store ADD COLUMN creator TEXT NOT NULL DEFAULT 'ai'",
            [],
        )?;
    }
    conn.execute(
        "UPDATE memory_store SET name = substr(trim(content), 1, 60) WHERE trim(name) = ''",
        [],
    )?;

    ensure_sync_metadata(conn)?;
    ensure_sync_runtime_state(conn)?;
    migrate_workspace_bindings(conn)?;
    migrate_explicit_memories(conn)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_permission_mode_to_existing_sessions() {
        unsafe {
            let _ = rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }

        let mut conn = Connection::open_in_memory().unwrap();
        let legacy_schema = crate::db::schema::SCHEMA
            .lines()
            .filter(|line| !line.contains("permission_mode TEXT"))
            .collect::<Vec<_>>()
            .join("\n");
        conn.execute_batch(&legacy_schema).unwrap();
        let column_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'permission_mode'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(column_count, 0);
        conn.execute(
            "INSERT INTO agents (id, name) VALUES ('agnes', 'Agnes')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, agent_id, title) VALUES ('legacy', 'agnes', 'Legacy')",
            [],
        )
        .unwrap();

        apply(&mut conn).unwrap();

        let mode: String = conn
            .query_row(
                "SELECT permission_mode FROM sessions WHERE id = 'legacy'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(mode, "auto");
    }

    #[test]
    fn adds_structured_memory_fields_to_existing_databases() {
        unsafe {
            let _ = rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }

        let mut conn = Connection::open_in_memory().unwrap();
        let legacy_schema = crate::db::schema::SCHEMA
            .lines()
            .filter(|line| {
                !line.contains("name TEXT NOT NULL DEFAULT ''")
                    && !line.contains("keywords TEXT")
                    && !line.contains("creator TEXT NOT NULL DEFAULT 'ai'")
            })
            .collect::<Vec<_>>()
            .join("\n");
        conn.execute_batch(&legacy_schema).unwrap();
        conn.execute(
            "INSERT INTO agents (id, name) VALUES ('agnes', 'Agnes')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO memory_store (id, agent_id, content) VALUES ('memory-1', 'agnes', 'Legacy memory content')",
            [],
        )
        .unwrap();

        apply(&mut conn).unwrap();

        let (name, keywords, creator): (String, Option<String>, String) = conn
            .query_row(
                "SELECT name, keywords, creator FROM memory_store WHERE id = 'memory-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(name, "Legacy memory content");
        assert!(keywords.is_none());
        assert_eq!(creator, "ai");
    }

    #[test]
    fn migrates_placeholder_documents_into_versioned_collections() {
        unsafe {
            let _ = rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }

        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE documents (\
               id TEXT PRIMARY KEY, agent_id TEXT NOT NULL, title TEXT, path TEXT, created_at TEXT);\
             CREATE TABLE document_chunks (\
               id TEXT PRIMARY KEY, document_id TEXT NOT NULL, content TEXT, embedding_id TEXT);\
             INSERT INTO documents (id, agent_id, title, path, created_at)\
               VALUES ('document-1', 'agnes', 'Legacy note', '/tmp/legacy.txt', '123');\
             INSERT INTO document_chunks (id, document_id, content)\
               VALUES ('chunk-1', 'document-1', 'A preserved legacy paragraph.');",
        )
        .unwrap();

        apply(&mut conn).unwrap();

        let row: (String, String, String, String, i64) = conn
            .query_row(
                "SELECT d.collection_id, d.current_version_id, v.plaintext_hash, b.local_path, c.token_count \
                 FROM documents d \
                 JOIN document_versions v ON v.id = d.current_version_id \
                 JOIN document_sources s ON s.document_id = d.id \
                 JOIN document_local_bindings b ON b.source_id = s.id \
                 JOIN document_chunks c ON c.document_version_id = v.id \
                 WHERE d.id = 'document-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .unwrap();
        assert_eq!(row.0, "legacy-collection:agnes");
        assert_eq!(row.1, "legacy-version:document-1:1");
        assert_eq!(row.2.len(), 64);
        assert_eq!(row.3, "/tmp/legacy.txt");
        assert!(row.4 > 0);

        let indexed: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM document_chunks_fts WHERE chunk_id = 'chunk-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(indexed, 1);
        assert!(has_column(&conn, "embedding_items", "collection_id"));
        assert!(!table_exists(&conn, "legacy_documents"));
        assert!(!table_exists(&conn, "legacy_document_chunks"));
    }

    #[test]
    fn migrates_legacy_explicit_memory_settings_to_entities() {
        unsafe {
            let _ = rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }

        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn).unwrap();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('agent:agnes:user_md', 'User profile')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('agent:agnes:memory_md', 'Stable memory')",
            [],
        )
        .unwrap();

        apply(&mut conn).unwrap();

        let rows: Vec<(String, String, String)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT id, kind, content FROM explicit_memories \
                     WHERE agent_id = 'agnes' ORDER BY kind",
                )
                .unwrap();
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].1, "memory_md");
        assert_eq!(rows[0].2, "Stable memory");
        assert_eq!(rows[1].1, "user_md");
        assert_eq!(rows[1].2, "User profile");
        assert!(rows
            .iter()
            .all(|(id, _, _)| uuid::Uuid::parse_str(id).is_ok()));
        let legacy_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM settings WHERE key LIKE 'agent:%:_md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(legacy_count, 0);
    }

    #[test]
    fn moves_legacy_workspace_paths_to_device_local_bindings() {
        unsafe {
            let _ = rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }

        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE workspaces ( \
               id TEXT PRIMARY KEY, agent_id TEXT NOT NULL, name TEXT, folder_path TEXT, \
               created_at TEXT, updated_at TEXT); \
             INSERT INTO workspaces \
               (id, agent_id, name, folder_path, created_at, updated_at) \
             VALUES ('workspace-1', 'agnes', 'Project', '/tmp/project', '1', '2');",
        )
        .unwrap();

        apply(&mut conn).unwrap();

        assert!(!has_column(&conn, "workspaces", "folder_path"));
        assert!(has_column(&conn, "workspaces", "version"));
        let binding: (String, String) = conn
            .query_row(
                "SELECT workspace_id, folder_path FROM workspace_bindings \
                 WHERE workspace_id = 'workspace-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(binding, ("workspace-1".into(), "/tmp/project".into()));
    }

    #[test]
    fn adds_sync_metadata_to_legacy_entity_tables() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE agents (id TEXT PRIMARY KEY); \
             CREATE TABLE sessions (id TEXT PRIMARY KEY); \
             CREATE TABLE messages (id TEXT PRIMARY KEY); \
             CREATE TABLE memory_store (id TEXT PRIMARY KEY); \
             CREATE TABLE workspaces (id TEXT PRIMARY KEY); \
             INSERT INTO agents (id) VALUES ('agent-1'); \
             INSERT INTO sessions (id) VALUES ('session-1'); \
             INSERT INTO messages (id) VALUES ('message-1'); \
             INSERT INTO memory_store (id) VALUES ('memory-1'); \
             INSERT INTO workspaces (id) VALUES ('workspace-1');",
        )
        .unwrap();

        ensure_sync_metadata(&conn).unwrap();

        for table in [
            "agents",
            "sessions",
            "messages",
            "memory_store",
            "workspaces",
        ] {
            assert!(has_column(&conn, table, "version"));
            assert!(has_column(&conn, table, "deleted_at"));
            assert!(has_column(&conn, table, "origin_device_id"));
            let version: i64 = conn
                .query_row(&format!("SELECT version FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(version, 1);
        }
    }

    #[test]
    fn sync_runtime_device_is_stable_and_recovers_in_flight_changes() {
        unsafe {
            let _ = rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn).unwrap();
        let first_device: String = conn
            .query_row(
                "SELECT device_id FROM sync_runtime_state WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(uuid::Uuid::parse_str(&first_device).is_ok());
        conn.execute(
            "INSERT INTO sync_outbox (change_id, device_id, entity_type, entity_id, operation, \
             local_version, hlc, payload_hash, status, created_at) \
             VALUES ('change-1', ?1, 'agent', 'agent-1', 'upsert', 1, \
                     '1-0000-device01', ?2, 'in_flight', 1)",
            params![first_device, "a".repeat(64)],
        )
        .unwrap();

        apply(&mut conn).unwrap();

        let (second_device, status): (String, String) = conn
            .query_row(
                "SELECT r.device_id, o.status FROM sync_runtime_state r \
                 JOIN sync_outbox o ON o.change_id = 'change-1' WHERE r.singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(second_device, first_device);
        assert_eq!(status, "pending");
    }

    #[test]
    fn adds_and_backfills_source_payload_for_legacy_outbox_rows() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE sync_outbox ( \
               change_id TEXT PRIMARY KEY, operation TEXT NOT NULL, payload TEXT, status TEXT); \
             CREATE TABLE sync_runtime_state (singleton INTEGER PRIMARY KEY, device_id TEXT); \
             INSERT INTO sync_outbox (change_id, operation, payload, status) \
             VALUES ('upsert-1', 'upsert', '{\"name\":\"legacy\"}', 'in_flight'), \
                    ('delete-1', 'delete', NULL, 'pending');",
        )
        .unwrap();

        ensure_sync_runtime_state(&conn).unwrap();

        assert!(has_column(&conn, "sync_outbox", "source_payload"));
        let rows: (Option<String>, Option<String>, String) = conn
            .query_row(
                "SELECT \
                   (SELECT source_payload FROM sync_outbox WHERE change_id = 'upsert-1'), \
                   (SELECT source_payload FROM sync_outbox WHERE change_id = 'delete-1'), \
                   (SELECT status FROM sync_outbox WHERE change_id = 'upsert-1')",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(rows.0.as_deref(), Some("{\"name\":\"legacy\"}"));
        assert_eq!(rows.1, None);
        assert_eq!(rows.2, "pending");
    }
}
