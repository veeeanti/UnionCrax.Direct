mod settings;
mod logging;
mod downloads;
mod games;
mod auth;
mod rpc;
mod updater;
mod tray;

use tauri::Manager;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_http::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_updater::Builder::default().build())
        .setup(|app| {
            // Initialize logging
            logging::init_logging(app.handle())?;

            // Initialize download directory
            downloads::ensure_download_dir(app.handle())?;

            // Set up system tray
            tray::setup_tray(app)?;

            // Initialize Discord RPC if enabled
            let settings = settings::read_settings(app.handle());
            let rpc_enabled = settings.get("discordRpcEnabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            if rpc_enabled {
                rpc::update_rpc_settings(true);
            }

            // Schedule update check
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                let _ = updater::check_for_updates_silent(&handle).await;
            });

            // Inject a script to patch fetch for cross-origin credentials
            if let Some(win) = app.get_webview_window("main") {
                let script = r#"
                    const _origFetch = window.fetch;
                    window.fetch = function(input, init) {
                        const url = typeof input === 'string' ? input : (input instanceof URL ? input.href : (input && input.url ? input.url : ''));
                        if (url && (url.includes('union-crax.xyz') || url.includes('localhost'))) {
                            init = Object.assign({}, init || {});
                            if (!init.credentials) init.credentials = 'include';
                        }
                        return _origFetch.call(this, input, init);
                    };
                "#;
                let _: Result<(), _> = win.eval(script);
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // Settings
            settings::setting_get,
            settings::setting_set,
            settings::setting_clear_all,
            settings::settings_export,
            settings::settings_import,
            // Logging
            logging::log_message,
            logging::logs_get,
            logging::logs_clear,
            logging::logs_open_folder,
            // Downloads
            downloads::download_start,
            downloads::download_cancel,
            downloads::download_pause,
            downloads::download_resume,
            downloads::download_resume_interrupted,
            downloads::download_resume_with_fresh_url,
            downloads::download_show,
            downloads::download_open,
            downloads::disk_list,
            downloads::download_path_get,
            downloads::download_path_set,
            downloads::download_path_pick,
            downloads::download_usage,
            downloads::download_cache_clear,
            downloads::installed_save,
            downloads::installed_update_metadata,
            downloads::installed_list,
            downloads::installed_get,
            downloads::installed_list_by_appid,
            downloads::installed_list_global,
            downloads::installed_get_global,
            downloads::installing_list,
            downloads::installing_get,
            downloads::installing_list_global,
            downloads::installing_get_global,
            downloads::installing_status_set,
            downloads::installing_delete,
            downloads::installed_delete,
            downloads::add_external_game,
            downloads::pick_external_game_folder,
            downloads::pick_image,
            downloads::network_test,
            // Games
            games::game_exe_list,
            games::game_subfolder_find,
            games::game_browse_exe,
            games::game_exe_launch,
            games::game_exe_launch_admin,
            games::game_exe_running,
            games::game_exe_quit,
            games::create_desktop_shortcut,
            games::delete_desktop_shortcut,
            // Auth
            auth::auth_login,
            auth::auth_logout,
            auth::auth_session,
            auth::auth_fetch,
            auth::auth_store_cookies,
            // RPC
            rpc::rpc_set_activity,
            rpc::rpc_clear,
            rpc::rpc_status,
            // Updater
            updater::check_for_updates,
            updater::update_retry,
            updater::get_version,
        ])
        .on_window_event(|window, event| {
            match event {
                tauri::WindowEvent::CloseRequested { api, .. } => {
                    // Hide to tray instead of closing
                    window.hide().unwrap_or_default();
                    api.prevent_close();
                    rpc::hide_rpc_activity();
                }
                tauri::WindowEvent::Focused(true) => {
                    rpc::restore_rpc_activity();
                }
                tauri::WindowEvent::Focused(false) => {}
                _ => {}
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
