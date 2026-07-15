#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent;
mod commands;
mod db;
mod error;
mod memory;
mod state;
mod tools;

use std::sync::Arc;

use tauri::Manager;

use crate::state::AppState;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // 1) 初始化 SQLite（阻塞直到建表完成）
            let data_dir = app
                .path()
                .app_data_dir()
                .expect("无法获取 app data 目录");
            std::fs::create_dir_all(&data_dir).ok();
            let db = db::spawn_db_actor(data_dir.join("agnes.db"));

            // 2) 启动 AgentManager：Rust 起 WS Server + 拉起 Python sidecar。
            //    非致命：失败仅日志，不阻断 UI 启动。
            let agent = Arc::new(agent::AgentManager::new());
            if let Err(e) = agent.start(db.clone(), app.handle().clone()) {
                eprintln!("[agent] 启动失败（非致命）：{e}");
            }

            app.manage(AppState { db, agent });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::ping,
            commands::list_agents,
            commands::update_agent_model,
            commands::upsert_agent,
            commands::delete_agent,
            commands::create_session,
            commands::list_sessions,
            commands::delete_session,
            commands::set_session_pin,
            commands::set_session_llm,
            commands::list_workspaces,
            commands::create_workspace,
            commands::rename_workspace,
            commands::delete_workspace,
            commands::rename_session,
            commands::get_debug_prompt,
            commands::list_messages,
            commands::switch_version,
            commands::create_branch,
            commands::delete_message,
            commands::edit_and_resend,
            commands::regenerate_message,
            commands::replace_message_parts,
            commands::cancel_run,
            commands::send_message,
            commands::approve_tool,
            commands::get_explicit_memories,
            commands::save_explicit_memories,
            commands::list_audit_logs,
            commands::list_providers,
            commands::upsert_provider,
            commands::delete_provider,
            commands::get_provider_api_key,
            commands::test_provider,
            commands::fetch_provider_models,
            commands::get_setting,
            commands::set_setting
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
