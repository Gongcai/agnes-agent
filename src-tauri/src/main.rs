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
            if let Err(e) = agent.start() {
                eprintln!("[agent] 启动失败（非致命）：{e}");
            }

            app.manage(AppState { db, agent });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![commands::ping, commands::list_agents])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
