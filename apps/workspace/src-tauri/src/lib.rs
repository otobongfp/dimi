mod commands;

use dimi_runtime::kernel::Runtime;
use std::sync::Arc;
use tauri::{Emitter, Manager};

fn forward_events_to_frontend(app: tauri::AppHandle, runtime: Arc<Runtime>) {
    let mut receiver = runtime.events.subscribe();
    tauri::async_runtime::spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    let _ = app.emit(&event.topic, &event.payload);
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let check = commands::compute_system_check();
            let continue_notify = Arc::new(tokio::sync::Notify::new());
            app.manage(Arc::new(commands::SystemCheckState {
                result: check.clone(),
                continue_notify: continue_notify.clone(),
            }));

            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if check.requires_confirmation {
                    continue_notify.notified().await;
                }

                match Runtime::boot().await {
                    Ok(runtime) => {
                        let runtime = Arc::new(runtime);
                        forward_events_to_frontend(app_handle.clone(), runtime.clone());
                        app_handle.manage(runtime);
                        let _ = app_handle.emit("dimi.ready", ());
                    }
                    Err(e) => {
                        tracing::error!("runtime boot failed: {e}");
                        let _ = app_handle.emit("dimi.boot_failed", e.to_string());
                    }
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::workspaces_create,
            commands::workspaces_list,
            commands::workspaces_load,
            commands::workspaces_update,
            commands::workspaces_delete,
            commands::repositories_add_folder,
            commands::repositories_add_files,
            commands::repositories_reindex,
            commands::repositories_indexing_status,
            commands::repositories_get,
            commands::documents_list,
            commands::conversations_create,
            commands::conversations_list,
            commands::conversations_attach_workspace,
            commands::conversations_delete,
            commands::messages_list,
            commands::chat_send,
            commands::models_list_installed,
            commands::models_list_available,
            commands::models_download,
            commands::models_active,
            commands::health_snapshot,
            commands::runtime_health,
            commands::plugins_discover,
            commands::plugins_list,
            commands::plugins_install,
            commands::plugins_enable,
            commands::plugins_disable,
            commands::settings_get_memory_budget,
            commands::settings_set_memory_budget,
            commands::resource_preflight_check,
            commands::resource_preflight_load_model,
            commands::system_check_status,
            commands::system_check_continue,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
