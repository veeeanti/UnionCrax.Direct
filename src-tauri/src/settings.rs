use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use once_cell::sync::Lazy;
use tauri::{AppHandle, Emitter, Manager};

static SETTINGS_CACHE: Lazy<Mutex<Option<HashMap<String, Value>>>> =
    Lazy::new(|| Mutex::new(None));

fn settings_path(app: &AppHandle) -> PathBuf {
    app.path().app_data_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("settings.json")
}

fn apply_defaults(mut settings: HashMap<String, Value>) -> HashMap<String, Value> {
    if !settings.contains_key("discordRpcEnabled") {
        settings.insert("discordRpcEnabled".to_string(), Value::Bool(true));
    }
    if !settings.contains_key("verboseDownloadLogging") {
        settings.insert("verboseDownloadLogging".to_string(), Value::Bool(false));
    }
    settings
}

pub fn read_settings(app: &AppHandle) -> HashMap<String, Value> {
    let mut cache = SETTINGS_CACHE.lock().unwrap();
    if let Some(ref s) = *cache {
        return s.clone();
    }
    let path = settings_path(app);
    let settings = if path.exists() {
        match std::fs::read_to_string(&path) {
            Ok(raw) => serde_json::from_str::<HashMap<String, Value>>(&raw).unwrap_or_default(),
            Err(_) => HashMap::new(),
        }
    } else {
        HashMap::new()
    };
    let with_defaults = apply_defaults(settings);
    *cache = Some(with_defaults.clone());
    // Write defaults back if needed
    let _ = write_settings_inner(&path, &with_defaults);
    with_defaults
}

fn write_settings_inner(path: &PathBuf, settings: &HashMap<String, Value>) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(settings)?;
    std::fs::write(path, json)?;
    Ok(())
}

pub fn write_settings(app: &AppHandle, settings: &HashMap<String, Value>) {
    let path = settings_path(app);
    let _ = write_settings_inner(&path, settings);
    let mut cache = SETTINGS_CACHE.lock().unwrap();
    *cache = Some(settings.clone());
}

fn broadcast_setting_change(app: &AppHandle, key: &str, value: &Value) {
    let payload = serde_json::json!({ "key": key, "value": value });
    let _ = app.emit("uc:setting-changed", payload);
}

// ── Tauri commands ──────────────────────────────────────────────────────────

#[tauri::command]
pub fn setting_get(app: AppHandle, key: String) -> Value {
    let settings = read_settings(&app);
    settings.get(&key).cloned().unwrap_or(Value::Null)
}

#[tauri::command]
pub fn setting_set(app: AppHandle, key: String, value: Value) -> Value {
    let mut settings = read_settings(&app);
    let prev = settings.clone();
    settings.insert(key.clone(), value.clone());
    write_settings(&app, &settings);
    // Handle discordRpcEnabled changes
    if key == "discordRpcEnabled" {
        let enabled = value.as_bool().unwrap_or(true);
        crate::rpc::update_rpc_settings(enabled);
    }
    // Broadcast changes
    if prev.get(&key) != settings.get(&key) {
        broadcast_setting_change(&app, &key, &value);
    }
    serde_json::json!({ "ok": true })
}

#[tauri::command]
pub fn setting_clear_all(app: AppHandle) -> Value {
    let defaults = apply_defaults(HashMap::new());
    write_settings(&app, &defaults);
    let _ = app.emit("uc:setting-changed", serde_json::json!({ "key": "__CLEAR_ALL__", "value": null }));
    serde_json::json!({ "ok": true })
}

#[tauri::command]
pub async fn settings_export(app: AppHandle) -> Value {
    use tauri_plugin_dialog::DialogExt;
    let settings = read_settings(&app);
    let default_name = format!("unioncrax-direct-settings-{}.json", chrono::Utc::now().timestamp_millis());
    let docs_dir = app.path().document_dir()
        .or_else(|_| app.path().download_dir())
        .unwrap_or_else(|_| PathBuf::from("."));

    let path = app.dialog()
        .file()
        .set_title("Export Settings")
        .set_file_name(&default_name)
        .add_filter("JSON", &["json"])
        .blocking_save_file();

    match path {
        Some(p) => {
            let path_buf = p.as_path().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from(p.to_string()));
            match serde_json::to_string_pretty(&settings) {
                Ok(json) => match std::fs::write(&path_buf, json) {
                    Ok(_) => serde_json::json!({ "ok": true, "path": path_buf.to_string_lossy() }),
                    Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
                },
                Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
            }
        }
        None => serde_json::json!({ "ok": false, "error": "cancelled" }),
    }
}

#[tauri::command]
pub async fn settings_import(app: AppHandle) -> Value {
    use tauri_plugin_dialog::DialogExt;
    let path = app.dialog()
        .file()
        .add_filter("JSON", &["json"])
        .blocking_pick_file();

    match path {
        Some(p) => {
            let path_buf = p.as_path().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from(p.to_string()));
            match std::fs::read_to_string(&path_buf) {
                Ok(raw) => match serde_json::from_str::<HashMap<String, Value>>(&raw) {
                    Ok(parsed) => {
                        let prev = read_settings(&app);
                        let next = apply_defaults(parsed);
                        write_settings(&app, &next);
                        // Broadcast all changed keys
                        for (k, v) in &next {
                            if prev.get(k) != Some(v) {
                                broadcast_setting_change(&app, k, v);
                            }
                        }
                        serde_json::json!({ "ok": true })
                    }
                    Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
                },
                Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
            }
        }
        None => serde_json::json!({ "ok": false, "error": "cancelled" }),
    }
}
