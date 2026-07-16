#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent;
mod commands;
mod db;
mod embeddings;
mod error;
mod memory;
mod model_registry;
mod secrets;
mod state;
pub mod sync;
mod tools;

use std::sync::Arc;

use tauri::Manager;

use crate::state::AppState;

fn main() {
    if let Some(exit_code) = tools::sandbox::run_sandbox_helper_if_requested() {
        std::process::exit(exit_code);
    }

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

            let secrets: secrets::SharedSecretStore = Arc::new(secrets::OsSecretStore::new());
            let secret_store_startup_error = match tauri::async_runtime::block_on(async {
                secrets::verify_secret_store(secrets.as_ref()).await?;
                secrets::migrate_legacy_provider_api_keys(&db, secrets.as_ref()).await
            }) {
                Ok(migrated) => {
                    if migrated > 0 {
                        println!(
                            "[secrets] Migrated {migrated} provider credential(s) to OS keyring"
                        );
                    }
                    None
                }
                Err(error) => {
                    eprintln!("[secrets] OS keyring initialization failed: {error}");
                    Some(error.to_string())
                }
            };
            let sync = Arc::new(
                sync::engine::SyncService::new(db.clone(), secrets.clone())
                    .expect("无法初始化同步服务"),
            );
            sync.clone().start_background();

            // 2) 启动 AgentManager：Rust 起 WS Server + 拉起 Python sidecar。
            //    非致命：失败仅日志，不阻断 UI 启动。
            let agent = Arc::new(agent::AgentManager::new());
            if let Err(e) = agent.start(db.clone(), app.handle().clone()) {
                eprintln!("[agent] 启动失败（非致命）：{e}");
            }

            app.manage(AppState {
                db,
                agent,
                secrets,
                sync,
                secret_store_startup_error,
            });
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
            commands::set_session_permission_mode,
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
            commands::list_memories,
            commands::get_memory_embedding_status,
            commands::vectorize_memories,
            commands::create_memory,
            commands::update_memory,
            commands::delete_memory,
            commands::list_audit_logs,
            commands::list_providers,
            commands::upsert_provider,
            commands::delete_provider,
            commands::get_model_roles,
            commands::set_model_roles,
            commands::get_secret_store_status,
            commands::test_provider,
            commands::fetch_provider_models,
            commands::get_setting,
            commands::set_setting,
            commands::get_sync_status,
            commands::list_sync_conflicts,
            commands::resolve_sync_conflict,
            commands::list_sync_devices,
            commands::revoke_sync_device,
            commands::sync_now,
            commands::set_sync_credential
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
