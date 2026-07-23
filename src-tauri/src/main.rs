#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent;
mod browser;
mod commands;
mod db;
mod document_parser;
mod embeddings;
mod error;
mod mcp;
mod memory;
mod model_registry;
mod notifications;
mod pdf_models;
mod reading;
mod reading_context_menu;
mod secrets;
mod skills;
mod state;
mod storage;
pub mod sync;
mod tools;
mod user_profile;
mod web;

use std::sync::Arc;

use tauri::{AppHandle, Manager, Runtime, WebviewWindow, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_global_shortcut::{
    Builder as GlobalShortcutBuilder, ShortcutEvent, ShortcutState,
};

use crate::state::AppState;

const QUICK_WINDOW_LABEL: &str = "quick";

#[cfg(target_os = "linux")]
fn configure_linux_webview_environment() {
    // WebKitGTK's DMA-BUF renderer can produce an empty surface on some Arch GPU stacks.
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
}

#[cfg(not(target_os = "linux"))]
fn configure_linux_webview_environment() {}

fn create_quick_window<R: Runtime>(app: &AppHandle<R>) -> Option<WebviewWindow<R>> {
    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|window| window.label == QUICK_WINDOW_LABEL)?;

    match WebviewWindowBuilder::from_config(app, config).and_then(|builder| builder.build()) {
        Ok(window) => Some(window),
        Err(error) => {
            eprintln!("[quick] 无法创建快速弹窗: {error}");
            None
        }
    }
}

#[cfg(target_os = "linux")]
fn configure_quick_popup<R: Runtime>(window: &WebviewWindow<R>) {
    use gdk::WindowTypeHint;
    use gtk::prelude::*;

    let Ok(gtk_window) = window.gtk_window() else {
        return;
    };

    // Utility keeps the X11 popup floating while retaining normal input focus handling.
    gtk_window.set_type_hint(WindowTypeHint::Utility);
    gtk_window.set_modal(false);
    gtk_window.set_skip_taskbar_hint(true);
    gtk_window.set_skip_pager_hint(true);
    gtk_window.set_keep_above(true);
    gtk_window.set_accept_focus(true);
    gtk_window.set_focus_on_map(true);
    gtk_window.set_role("agnes-quick");
    gtk_window.realize();
    if let Some(gdk_window) = gtk_window.window() {
        gdk_window.set_type_hint(WindowTypeHint::Utility);
        gdk_window.set_skip_taskbar_hint(true);
        gdk_window.set_skip_pager_hint(true);
        gdk_window.set_keep_above(true);
        gdk_window.set_accept_focus(true);
    }
}

#[cfg(not(target_os = "linux"))]
fn configure_quick_popup<R: Runtime>(_window: &WebviewWindow<R>) {}

#[cfg(target_os = "linux")]
fn focus_quick_popup<R: Runtime>(window: &WebviewWindow<R>) {
    let popup = window.clone();
    let _ = window.run_on_main_thread(move || {
        use gtk::prelude::*;
        use std::time::Duration;

        let Ok(gtk_window) = popup.gtk_window() else {
            return;
        };
        let request_focus = move |popup: &gtk::ApplicationWindow| {
            let Some(gdk_window) = popup.window() else {
                return;
            };
            let Ok(x11_window) = gdk_window.clone().downcast::<gdkx11::X11Window>() else {
                return;
            };
            let timestamp = gdkx11::functions::x11_get_server_time(&x11_window);
            x11_window.set_user_time(timestamp);
            popup.present_with_time(timestamp);
            gdk_window.focus(timestamp);
            unsafe {
                let display = (x11::xlib::XOpenDisplay)(std::ptr::null());
                if !display.is_null() {
                    (x11::xlib::XSetInputFocus)(
                        display,
                        x11_window.xid(),
                        x11::xlib::RevertToParent,
                        timestamp.into(),
                    );
                    (x11::xlib::XFlush)(display);
                    (x11::xlib::XCloseDisplay)(display);
                }
            }
        };

        request_focus(&gtk_window);
        gtk::glib::timeout_add_local_once(Duration::from_millis(80), move || {
            if gtk_window.is_visible() {
                request_focus(&gtk_window);
            }
        });
    });
}

#[cfg(not(target_os = "linux"))]
fn focus_quick_popup<R: Runtime>(_window: &WebviewWindow<R>) {}

fn reveal_quick_popup<R: Runtime>(window: &WebviewWindow<R>) {
    let _ = window.center();
    let _ = window.show();
    let _ = window.set_focus();
    focus_quick_popup(window);
}

fn toggle_quick_popup<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(QUICK_WINDOW_LABEL) {
        match window.is_visible() {
            Ok(true) => {
                let _ = window.hide();
            }
            Ok(false) => reveal_quick_popup(&window),
            Err(error) => eprintln!("[quick] 无法读取快速弹窗状态: {error}"),
        }
        return;
    }

    // A compositor-level kill can destroy a native window without a close event.
    // Recreate the hidden instance on the main thread so the global shortcut stays usable.
    let app_handle = app.clone();
    let task_handle = app_handle.clone();
    let _ = app_handle.run_on_main_thread(move || {
        let Some(window) = create_quick_window(&task_handle) else {
            return;
        };
        configure_quick_popup(&window);
        reveal_quick_popup(&window);
    });
}

fn main() {
    if let Some(exit_code) = tools::sandbox::run_sandbox_helper_if_requested() {
        std::process::exit(exit_code);
    }

    configure_linux_webview_environment();

    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(
            GlobalShortcutBuilder::new()
                .with_shortcut("CommandOrControl+Shift+Space")
                .expect("无法注册快速窗口快捷键")
                .with_handler(|app, _shortcut, event: ShortcutEvent| {
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }
                    toggle_quick_popup(app);
                })
                .build(),
        )
        .on_window_event(|window, event| {
            if window.label() != QUICK_WINDOW_LABEL {
                return;
            }
            if let WindowEvent::CloseRequested { api, .. } = event {
                // The quick popup is a resident singleton. Close means hide, never destroy.
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .setup(|app| {
            // 1) 初始化 SQLite（阻塞直到建表完成）
            let data_dir = app.path().app_data_dir().expect("无法获取 app data 目录");
            std::fs::create_dir_all(&data_dir).ok();
            let app_local_data_dir = app
                .path()
                .app_local_data_dir()
                .expect("无法获取本机应用数据目录");
            let document_dir = app.path().document_dir().ok();
            let home_workspace_dir = tools::workspace::prepare_home_workspace(
                document_dir.as_deref(),
                &app_local_data_dir,
            )
            .expect("无法创建 Home 默认工作区");
            let db = db::spawn_db_actor(data_dir.join("agnes.db"));
            match tauri::async_runtime::block_on(db.recover_interrupted_assistants()) {
                Ok(0) => {}
                Ok(count) => {
                    eprintln!("[agent] Recovered {count} interrupted assistant response(s)")
                }
                Err(error) => eprintln!("[agent] Failed to recover interrupted responses: {error}"),
            }

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
            let notifications = Arc::new(notifications::NotificationService::new(
                db.clone(),
                app.handle().clone(),
            ));
            notifications.clone().start_background();
            let mcp = Arc::new(mcp::McpManager::new(db.clone(), secrets.clone()));
            let storage_registry = Arc::new(storage::StorageProviderRegistry::new());
            storage_registry
                .register(Arc::new(
                    storage::GoogleDriveFactory::new().expect("无法初始化 Google Drive Provider"),
                ))
                .expect("无法注册 Google Drive Provider");
            storage_registry
                .register(Arc::new(
                    storage::QuarkDriveFactory::new().expect("无法初始化夸克网盘 Provider"),
                ))
                .expect("无法注册夸克网盘 Provider");
            storage_registry
                .register(Arc::new(
                    storage::R2Factory::new().expect("无法初始化 R2 Worker Provider"),
                ))
                .expect("无法注册 R2 Worker Provider");
            let storage_credentials = Arc::new(storage::KeyringProviderCredentialStore::new(
                secrets.clone(),
            ));
            let sync_credentials =
                Arc::new(storage::SyncProviderCredentialAccess::new(secrets.clone()));
            let storage = Arc::new(storage::StorageService::new(
                db.clone(),
                storage_registry,
                storage_credentials,
                sync_credentials,
            ));
            if let Err(error) = tauri::async_runtime::block_on(
                storage.ensure_managed_r2_account(sync::engine::SYNC_GATEWAY_URL),
            ) {
                eprintln!("[storage] managed R2 account bootstrap failed: {error}");
            }
            sync.clone()
                .start_artifact_replication_background(storage.clone(), data_dir.join("artifacts"));
            storage::artifact_cache::start_background(db.clone(), data_dir.join("artifacts"));

            // 2) 启动 AgentManager：Rust 起 WS Server + 拉起 Python sidecar。
            //    非致命：失败仅日志，不阻断 UI 启动。
            let agent = Arc::new(agent::AgentManager::new());
            if let Err(e) = agent.start(
                db.clone(),
                app.handle().clone(),
                mcp.clone(),
                secrets.clone(),
                home_workspace_dir.clone(),
            ) {
                eprintln!("[agent] 启动失败（非致命）：{e}");
            }

            #[cfg(target_os = "linux")]
            if let Some(window) = app.get_webview_window("main") {
                if let Err(error) = reading_context_menu::install(&window, app.handle().clone()) {
                    eprintln!("[reading] 无法安装原生右键菜单拦截：{error}");
                }
            }

            let pdf_models = Arc::new(pdf_models::PdfModelPackageManager::new(&data_dir));
            app.manage(AppState {
                app_handle: app.handle().clone(),
                data_dir,
                home_workspace_dir,
                db,
                agent,
                mcp,
                secrets,
                storage,
                sync,
                notifications,
                document_parser: Arc::new(document_parser::DocumentParserManager::default()),
                pdf_models,
                secret_store_startup_error,
            });

            if let Some(window) = create_quick_window(app.handle()) {
                configure_quick_popup(&window);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::ping,
            commands::open_webview_devtools,
            commands::list_agents,
            commands::update_agent_model,
            commands::upsert_agent,
            commands::delete_agent,
            commands::create_session,
            commands::list_sessions,
            commands::delete_session,
            commands::set_session_pin,
            commands::set_session_llm,
            commands::set_session_compress_threshold,
            commands::set_session_permission_mode,
            commands::list_workspaces,
            commands::create_workspace,
            commands::rename_workspace,
            commands::delete_workspace,
            commands::rename_session,
            commands::get_debug_prompt,
            commands::list_messages,
            commands::get_token_usage_stats,
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
            commands::get_user_profile,
            commands::save_user_profile,
            commands::get_agent_user_profile_inheritance,
            commands::set_agent_user_profile_inheritance,
            commands::list_memories,
            commands::get_memory_embedding_status,
            commands::vectorize_memories,
            commands::create_memory,
            commands::update_memory,
            commands::delete_memory,
            commands::list_reading_books,
            commands::publish_reading_epub,
            commands::import_reading_book,
            commands::open_reading_book_conversation,
            commands::list_reading_book_conversations,
            commands::select_reading_book_conversation,
            commands::new_reading_book_conversation,
            commands::update_reading_book_mode,
            commands::set_reading_book_content_context_allowed,
            commands::set_reading_context_menu_active,
            commands::update_reading_book_progress,
            commands::list_reading_highlights,
            commands::create_reading_highlight,
            commands::update_reading_highlight,
            commands::delete_reading_highlight,
            commands::list_knowledge_collections,
            commands::create_knowledge_collection,
            commands::list_knowledge_documents,
            commands::import_local_knowledge_document,
            commands::cancel_knowledge_import,
            commands::get_pdf_model_package_status,
            commands::install_pdf_model_package,
            commands::remove_pdf_model_package,
            commands::import_storage_knowledge_document,
            commands::search_knowledge,
            commands::vectorize_knowledge,
            commands::publish_knowledge_artifact,
            commands::get_knowledge_artifact_coverage,
            commands::search_knowledge_hybrid,
            commands::list_storage_provider_catalog,
            commands::list_storage_accounts,
            commands::authorize_storage_provider,
            commands::begin_storage_provider_authorization,
            commands::poll_storage_provider_authorization,
            commands::list_storage_files,
            commands::search_storage_files,
            commands::download_storage_file,
            commands::import_storage_reading_book,
            commands::download_storage_folder,
            commands::upload_storage_file,
            commands::trash_storage_files,
            commands::move_storage_files,
            commands::refresh_storage_quota,
            commands::list_storage_transfers,
            commands::remove_storage_account,
            commands::get_artifact_storage_status,
            commands::set_artifact_storage_quota,
            commands::cleanup_artifact_storage,
            commands::list_calendars,
            commands::create_calendar,
            commands::list_calendar_events,
            commands::get_calendar_event,
            commands::create_calendar_event,
            commands::update_calendar_event,
            commands::update_calendar_occurrence,
            commands::cancel_calendar_occurrence,
            commands::restore_calendar_occurrence,
            commands::delete_calendar_event,
            commands::list_task_lists,
            commands::create_task_list,
            commands::list_tasks,
            commands::list_all_tasks,
            commands::create_task,
            commands::complete_task,
            commands::update_task,
            commands::delete_task,
            commands::list_notifications,
            commands::mark_notification_read,
            commands::mark_all_notifications_read,
            commands::list_audit_logs,
            commands::list_providers,
            commands::upsert_provider,
            commands::delete_provider,
            commands::get_model_roles,
            commands::set_model_roles,
            commands::get_search_provider_settings,
            commands::set_search_provider_settings,
            commands::test_search_provider,
            commands::list_installed_skills,
            commands::install_skills_from_path,
            commands::install_skills_from_git,
            commands::set_skill_enabled,
            commands::uninstall_skill,
            commands::open_skill_directory,
            commands::get_secret_store_status,
            commands::test_provider,
            commands::fetch_provider_models,
            commands::get_setting,
            commands::set_setting,
            mcp::list_mcp_servers,
            mcp::upsert_mcp_server,
            mcp::delete_mcp_server,
            mcp::test_mcp_server,
            commands::get_sync_status,
            commands::list_sync_conflicts,
            commands::resolve_sync_conflict,
            commands::list_sync_devices,
            commands::revoke_sync_device,
            commands::sync_now,
            commands::set_sync_credential,
            commands::begin_sync_e2ee_setup,
            commands::begin_sync_e2ee_rotation,
            commands::confirm_sync_e2ee_setup,
            commands::restore_sync_e2ee,
            commands::discard_sync_e2ee_setup,
            commands::start_sync_pairing,
            commands::get_sync_pairing_request,
            commands::approve_sync_pairing,
            commands::join_sync_pairing,
            commands::finish_sync_pairing
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
