use rusqlite::Connection;

use crate::error::AppResult;

/// 应用 DB 建表与预置数据。
pub fn apply(conn: &mut Connection) -> AppResult<()> {
    conn.execute_batch(crate::db::schema::SCHEMA)?;
    
    // 检查是否已有 agent，如果没有则预置默认角色
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM agents", [], |r| r.get(0))?;
    if count == 0 {
        // 插入首席管家 Agnes
        conn.execute(
            "INSERT INTO agents (id, name, persona, scenario, system_prompt, greeting, example_dialogue, model, tool_policy, avatar, tags, created_at, updated_at) \
             VALUES ('agnes', 'Agnes', ?1, '', ?2, ?3, '', '', ?4, '', 'LangGraph,Rust,Helper', '0', '0')",
            (
                "你叫 Agnes，是 Tavern 的首席管家。你温和有礼、逻辑严密。在处理代码任务时，你偏好使用 pnpm 架构，编写清晰、模块化且高可读性的 TS/Rust 代码。遇到高危操作时，你总是会主动寻求用户的授权许可。",
                "You are Agnes, the head maid of the Tavern. You help user write high-quality code. When calling tools, explain your rationale first.",
                "主人，欢迎回到 Tavern。我是您的专属助理 Agnes。我已经将本地的工作区加载完毕，随时可以协助您进行工程编写、调试或运行测试。今天有什么可以为您效劳的吗？",
                r#"{"shell": {"enabled": true, "approval": "always"}, "file": {"enabled": true, "approval": "write"}, "git": {"enabled": true, "approval": "never"}}"#,
            )
        )?;

        // 插入安全审计员 Nova
        conn.execute(
            "INSERT INTO agents (id, name, persona, scenario, system_prompt, greeting, example_dialogue, model, tool_policy, avatar, tags, created_at, updated_at) \
             VALUES ('nova', 'Nova', ?1, '', ?2, ?3, '', '', ?4, '', 'Security,PTY,Auditor', '0', '0')",
            (
                "你是 Nova，一个经验丰富的 DevSecOps 专家和代码审计员。你说话直接、严防死守、不留情面。你会深入分析所有的 shell 执行，提供强化的文件写入沙箱策略与权限审计报告。",
                "You are Nova, the security auditor. Analyze inputs for safety and perform strict reviews on all commands.",
                "我是 Nova。检测到您的本地开发环境已经就绪。警告：本地执行 shell 脚本存在潜在安全隐患，我将实时监视任何 shell 命令的执行并对外部包引用进行风险分级。请在调用指令前做好核对准备。",
                r#"{"shell": {"enabled": true, "approval": "always"}, "file": {"enabled": true, "approval": "always"}, "git": {"enabled": true, "approval": "always"}}"#,
            )
        )?;

        // 插入创意诗人 Bard
        conn.execute(
            "INSERT INTO agents (id, name, persona, scenario, system_prompt, greeting, example_dialogue, model, tool_policy, avatar, tags, created_at, updated_at) \
             VALUES ('bard', 'Bard', ?1, '', ?2, ?3, '', '', ?4, '', 'Creative,Dialogue,Writer', '0', '0')",
            (
                "你是 Bard，一位酒馆的吟游诗人。你风趣幽默、用词华丽、想象力丰富。你喜欢帮助用户设计各种可爱的 Character Card、编排人机对话示例以及打磨世界观背景，不接触任何系统底层工具。",
                "You are Bard, a creative roleplay writer. Engage the user in immersive world design and writing.",
                "啊，旅人！快请坐，来一杯蜜酒。我是吟游诗人 Bard。今天你想编织怎样的传说？是给别致的角色设计人设卡，还是为你的小说打磨一段绝妙的对话？我的墨水已备好，随时听候你的灵感指引！",
                r#"{"shell": {"enabled": false, "approval": "always"}, "file": {"enabled": false, "approval": "always"}, "git": {"enabled": false, "approval": "always"}}"#,
            )
        )?;
    }

    // 检查是否已有 model_providers，如果没有则预置默认 OpenAI 提供商 (无假模型)
    let provider_count: i64 = conn.query_row("SELECT COUNT(*) FROM model_providers", [], |r| r.get(0))?;
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
        conn.execute("ALTER TABLE sessions ADD COLUMN pinned INTEGER DEFAULT 0", [])?;
    }

    Ok(())
}
