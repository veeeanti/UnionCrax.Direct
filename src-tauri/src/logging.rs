use std::path::PathBuf;
use std::sync::Mutex;
use once_cell::sync::Lazy;
use tauri::{AppHandle, Manager};
use serde_json::Value;

static LOG_SESSION_ID: Lazy<String> = Lazy::new(|| {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().subsec_nanos();
    format!("{:x}", t)
});

static LOG_MUTEX: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

fn log_path(app: &AppHandle) -> PathBuf {
    app.path().app_data_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("app-logs.txt")
}

pub fn init_logging(app: &AppHandle) -> anyhow::Result<()> {
    let path = log_path(app);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let header = format!(
        "[{}] [INFO ] === App Log Started (session {}, pid {}) ===\n",
        chrono::Utc::now().to_rfc3339(),
        *LOG_SESSION_ID,
        std::process::id()
    );
    std::fs::write(&path, header)?;
    Ok(())
}

pub fn uc_log(app: &AppHandle, message: &str, level: &str, data: Option<&Value>) {
    let _guard = LOG_MUTEX.lock().unwrap();
    let timestamp = chrono::Utc::now().to_rfc3339();
    let level_tag = format!("{:<5}", level.to_uppercase());
    let data_str = match data {
        Some(d) => format!(" | Data: {}", d),
        None => String::new(),
    };
    let log_line = format!("[{}] [{}] {}{}\n", timestamp, level_tag, message, data_str);

    let path = log_path(app);
    let _ = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&path)
        .and_then(|mut f| {
            use std::io::Write;
            f.write_all(log_line.as_bytes())
        });

    match level {
        "error" => eprintln!("[UC] [ERROR] {} {}", message, data_str),
        "warn" => eprintln!("[UC] [WARN] {} {}", message, data_str),
        _ => println!("[UC] [{}] {} {}", level.to_uppercase(), message, data_str),
    }
}

pub fn get_logs(app: &AppHandle) -> String {
    let path = log_path(app);
    std::fs::read_to_string(&path).unwrap_or_default()
}

pub fn clear_logs(app: &AppHandle) {
    let path = log_path(app);
    let header = format!(
        "[{}] [INFO ] === App Log Started (session {}, pid {}) ===\n",
        chrono::Utc::now().to_rfc3339(),
        *LOG_SESSION_ID,
        std::process::id()
    );
    let _ = std::fs::write(&path, header);
}

// ── Tauri commands ──────────────────────────────────────────────────────────

#[tauri::command]
pub fn log_message(app: AppHandle, level: String, message: String, data: Option<Value>) -> Value {
    uc_log(&app, &message, &level, data.as_ref());
    serde_json::json!({ "ok": true })
}

#[tauri::command]
pub fn logs_get(app: AppHandle) -> String {
    get_logs(&app)
}

#[tauri::command]
pub fn logs_clear(app: AppHandle) -> Value {
    clear_logs(&app);
    serde_json::json!({ "ok": true })
}

#[tauri::command]
pub async fn logs_open_folder(app: AppHandle) -> Value {
    use tauri_plugin_opener::OpenerExt;
    let path = log_path(&app);
    let folder = path.parent().unwrap_or(&path).to_string_lossy().to_string();
    match app.opener().open_path(&folder, None::<&str>) {
        Ok(_) => serde_json::json!({ "ok": true }),
        Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
    }
}
