/// Games module — game executable discovery, launching, and desktop shortcuts.
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::AppHandle;

use crate::downloads::list_download_roots;
use crate::logging::uc_log;
use crate::settings::read_settings;

const INSTALLED_DIR: &str = "installed";
const INSTALLED_MANIFEST: &str = "installed.json";

// ── Running games state ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunningGame {
    pub appid: Option<String>,
    #[serde(rename = "exePath")]
    pub exe_path: Option<String>,
    #[serde(rename = "gameName")]
    pub game_name: Option<String>,
    pub pid: u32,
    #[serde(rename = "startedAt")]
    pub started_at: i64,
}

static RUNNING_GAMES: Lazy<Mutex<HashMap<String, RunningGame>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

// ── Executable discovery ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExeInfo {
    pub name: String,
    pub path: String,
    pub depth: usize,
    pub size: u64,
}

fn is_linux_executable(path: &Path) -> bool {
    let lower = path.to_string_lossy().to_lowercase();
    if lower.ends_with(".desktop") || lower.ends_with(".dll") || lower.ends_with(".so") {
        return false;
    }
    if lower.ends_with(".appimage") || lower.ends_with(".sh") || lower.ends_with(".run")
        || lower.ends_with(".bin") || lower.ends_with(".x86_64") || lower.ends_with(".x86")
        || lower.ends_with(".exe")
    {
        return true;
    }
    // Check execute bit
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            return meta.permissions().mode() & 0o111 != 0;
        }
    }
    false
}

pub fn list_executables(root_dir: &Path, max_depth: usize, max_results: usize) -> Vec<ExeInfo> {
    let mut results = Vec::new();
    if !root_dir.exists() {
        return results;
    }

    let skip_dirs: HashSet<&str> = [
        "_redist", "__redist", "_commonredist", "directx", "$pluginsdir",
        "__support", "mono", ".mono",
    ].iter().cloned().collect();

    let mut pending = vec![(root_dir.to_path_buf(), 0usize)];
    let mut visited: HashSet<PathBuf> = HashSet::new();

    while let Some((cur, depth)) = pending.pop() {
        let entries = match std::fs::read_dir(&cur) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let normalized = path.to_string_lossy().to_lowercase();
            if visited.contains(&path) { continue; }
            visited.insert(path.clone());

            let file_type = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };

            if file_type.is_dir() || (file_type.is_symlink() && path.is_dir()) {
                let dir_name = entry.file_name().to_string_lossy().to_lowercase();
                if skip_dirs.contains(dir_name.as_str()) { continue; }
                if depth < max_depth {
                    pending.push((path, depth + 1));
                }
                continue;
            }

            if !file_type.is_file() && !file_type.is_symlink() { continue; }

            let name = entry.file_name().to_string_lossy().to_string();
            let relative_depth = path.strip_prefix(root_dir)
                .map(|p| p.components().count().saturating_sub(1))
                .unwrap_or(depth);
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);

            #[cfg(target_os = "windows")]
            {
                if normalized.ends_with(".exe") {
                    results.push(ExeInfo { name, path: path.to_string_lossy().to_string(), depth: relative_depth, size });
                }
            }
            #[cfg(not(target_os = "windows"))]
            {
                if is_linux_executable(&path) {
                    results.push(ExeInfo { name, path: path.to_string_lossy().to_string(), depth: relative_depth, size });
                }
            }
        }
    }

    results.sort_by(|a, b| {
        a.depth.cmp(&b.depth)
            .then(b.size.cmp(&a.size))
            .then(a.name.cmp(&b.name))
    });
    results.truncate(max_results.max(1));
    results
}

fn read_json_file(path: &Path) -> Option<Value> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn iterate_game_folders(root: &Path) -> Vec<(PathBuf, String)> {
    let mut results = Vec::new();
    if !root.exists() { return results; }
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                let folder = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                results.push((folder.clone(), name.clone()));
                let versions_dir = folder.join("versions");
                if versions_dir.exists() {
                    if let Ok(v_entries) = std::fs::read_dir(&versions_dir) {
                        for v_entry in v_entries.flatten() {
                            if v_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                                results.push((v_entry.path(), v_entry.file_name().to_string_lossy().to_string()));
                            }
                        }
                    }
                }
            }
        }
    }
    results
}

fn find_installed_folder(app: &AppHandle, appid: &str) -> Option<PathBuf> {
    for root in list_download_roots(app) {
        let installed_root = root.join(INSTALLED_DIR);
        for (folder, name) in iterate_game_folders(&installed_root) {
            let manifest_path = folder.join(INSTALLED_MANIFEST);
            if let Some(manifest) = read_json_file(&manifest_path) {
                if manifest.get("appid").and_then(|v| v.as_str()) == Some(appid) {
                    return Some(folder);
                }
            }
            if name == appid { return Some(folder); }
        }
    }
    None
}

// ── Launch helpers ────────────────────────────────────────────────────────────

fn resolve_launch_command(exe_path: &Path, settings: &HashMap<String, Value>) -> (String, Vec<String>, PathBuf) {
    let cwd = exe_path.parent().unwrap_or(exe_path).to_path_buf();

    #[cfg(not(target_os = "linux"))]
    {
        return (exe_path.to_string_lossy().to_string(), vec![], cwd);
    }

    #[cfg(target_os = "linux")]
    {
        let mode = settings.get("linuxLaunchMode")
            .and_then(|v| v.as_str())
            .unwrap_or("auto")
            .to_lowercase();
        let wine_path = settings.get("linuxWinePath")
            .and_then(|v| v.as_str())
            .unwrap_or("wine")
            .to_string();
        let proton_path = settings.get("linuxProtonPath")
            .and_then(|v| v.as_str())
            .unwrap_or("proton")
            .to_string();
        let is_exe = exe_path.to_string_lossy().to_lowercase().ends_with(".exe");

        if mode == "native" {
            return (exe_path.to_string_lossy().to_string(), vec![], cwd);
        }
        if is_exe {
            if mode == "proton" {
                return (proton_path, vec!["run".to_string(), exe_path.to_string_lossy().to_string()], cwd);
            }
            if mode == "wine" || mode == "auto" {
                return (wine_path, vec![exe_path.to_string_lossy().to_string()], cwd);
            }
        }
        (exe_path.to_string_lossy().to_string(), vec![], cwd)
    }
}

async fn is_process_running(pid: u32) -> bool {
    #[cfg(target_os = "windows")]
    {
        let output = tokio::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {}", pid)])
            .output()
            .await;
        match output {
            Ok(o) => String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()),
            Err(_) => false,
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        // Send signal 0 to check if process exists
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
}

async fn kill_process_tree(pid: u32) -> bool {
    #[cfg(target_os = "windows")]
    {
        let result = tokio::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output()
            .await;
        result.map(|o| o.status.success()).unwrap_or(false)
    }
    #[cfg(not(target_os = "windows"))]
    {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGTERM) == 0
                || libc::kill(pid as i32, libc::SIGTERM) == 0
        }
    }
}

// ── Desktop shortcuts ─────────────────────────────────────────────────────────

fn sanitize_desktop_filename(name: &str) -> String {
    let cleaned: String = name.chars()
        .filter(|c| !r#"\/:*?"<>|"#.contains(*c))
        .collect();
    let trimmed = cleaned.trim().to_string();
    if trimmed.is_empty() { "UnionCrax-Game".to_string() } else { trimmed }
}

// ── Tauri commands ────────────────────────────────────────────────────────────

#[tauri::command]
pub fn game_exe_list(app: AppHandle, appid: String, version_label: Option<String>) -> Value {
    let folder = find_installed_folder(&app, &appid);
    let resolved_folder = match folder {
        Some(f) => f,
        None => return serde_json::json!({ "ok": false, "error": "not-found", "exes": [] }),
    };

    let mut exes = list_executables(&resolved_folder, 6, 100);
    let mut effective_folder = resolved_folder.clone();

    // If no exes found, check single subfolder wrapper
    if exes.is_empty() {
        if let Ok(entries) = std::fs::read_dir(&resolved_folder) {
            let entries: Vec<_> = entries.flatten().collect();
            let subdirs: Vec<_> = entries.iter().filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false)).collect();
            let files: Vec<_> = entries.iter().filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false)).collect();
            if subdirs.len() == 1 && files.iter().all(|f| f.file_name().to_string_lossy() == INSTALLED_MANIFEST) {
                let sub_path = subdirs[0].path();
                let sub_exes = list_executables(&sub_path, 6, 100);
                if !sub_exes.is_empty() {
                    exes = sub_exes;
                    effective_folder = sub_path;
                }
            }
        }
    }

    // For external games, try externalPath
    if exes.is_empty() {
        let manifest_path = resolved_folder.join(INSTALLED_MANIFEST);
        if let Some(manifest) = read_json_file(&manifest_path) {
            let ext_path = manifest.get("externalPath")
                .or_else(|| manifest.get("metadata").and_then(|m| m.get("externalPath")))
                .and_then(|v| v.as_str())
                .map(PathBuf::from);
            if let Some(ext) = ext_path {
                if ext.exists() {
                    let ext_exes = list_executables(&ext, 6, 100);
                    return serde_json::json!({
                        "ok": true,
                        "folder": ext.to_string_lossy(),
                        "exes": ext_exes
                    });
                }
            }
        }
    }

    serde_json::json!({
        "ok": true,
        "folder": effective_folder.to_string_lossy(),
        "exes": exes
    })
}

#[tauri::command]
pub fn game_subfolder_find(folder: String) -> Option<String> {
    let path = PathBuf::from(&folder);
    if !path.exists() { return None; }

    if let Ok(entries) = std::fs::read_dir(&path) {
        let entries: Vec<_> = entries.flatten().collect();
        let subdirs: Vec<_> = entries.iter().filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false)).collect();
        let files: Vec<_> = entries.iter().filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false)).collect();
        if files.len() == 1 && files[0].file_name().to_string_lossy() == INSTALLED_MANIFEST && subdirs.len() == 1 {
            return Some(subdirs[0].path().to_string_lossy().to_string());
        }
    }
    None
}

#[tauri::command]
pub async fn game_browse_exe(app: AppHandle, default_path: Option<String>) -> Value {
    use tauri_plugin_dialog::DialogExt;
    let mut dialog = app.dialog().file().set_title("Select game executable");

    #[cfg(target_os = "windows")]
    {
        dialog = dialog.add_filter("Executables", &["exe"]);
    }

    let path = dialog.blocking_pick_file();
    match path {
        Some(p) => {
            let path_str = p.as_path().map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|| p.to_string());
            serde_json::json!({ "ok": true, "path": path_str })
        }
        None => serde_json::json!({ "ok": false }),
    }
}

#[tauri::command]
pub async fn game_exe_launch(
    app: AppHandle,
    appid: String,
    exe_path: String,
    game_name: Option<String>,
    show_game_name: Option<bool>,
) -> Value {
    let exe = PathBuf::from(&exe_path);
    if !exe.exists() {
        return serde_json::json!({ "ok": false, "error": "executable not found" });
    }

    let settings = read_settings(&app);
    let (command, args, cwd) = resolve_launch_command(&exe, &settings);

    uc_log(&app, &format!("Launching game: {} at {}", appid, exe_path), "info", None);

    #[cfg(target_os = "windows")]
    {
        let mut env = std::collections::HashMap::new();
        env.insert("PATH".to_string(), format!("{};{}", cwd.to_string_lossy(), std::env::var("PATH").unwrap_or_default()));

        let quote = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
        let cmd_line = std::iter::once(quote(&command))
            .chain(args.iter().map(|a| quote(a)))
            .collect::<Vec<_>>()
            .join(" ");

        match std::process::Command::new("cmd.exe")
            .args(["/d", "/s", "/c", &cmd_line])
            .current_dir(&cwd)
            .envs(&env)
            .spawn()
        {
            Ok(child) => {
                let pid = child.id();
                register_running_game(&app, &appid, &exe_path, pid, game_name.as_deref(), show_game_name.unwrap_or(true));
                uc_log(&app, &format!("Game launched: {} (PID: {})", appid, pid), "info", None);
                return serde_json::json!({ "ok": true, "pid": pid });
            }
            Err(e) => {
                uc_log(&app, &format!("Game launch failed: {} - {}", appid, e), "error", None);
                return serde_json::json!({ "ok": false, "error": e.to_string() });
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        match std::process::Command::new(&command)
            .args(&args)
            .current_dir(&cwd)
            .spawn()
        {
            Ok(child) => {
                let pid = child.id();
                register_running_game(&app, &appid, &exe_path, pid, game_name.as_deref(), show_game_name.unwrap_or(true));
                serde_json::json!({ "ok": true, "pid": pid })
            }
            Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
        }
    }
}

#[tauri::command]
pub async fn game_exe_launch_admin(
    app: AppHandle,
    appid: String,
    exe_path: String,
    game_name: Option<String>,
    show_game_name: Option<bool>,
) -> Value {
    #[cfg(not(target_os = "windows"))]
    {
        // Non-Windows: fall back to regular launch
        return game_exe_launch(app, appid, exe_path, game_name, show_game_name).await;
    }

    #[cfg(target_os = "windows")]
    {
        let exe = PathBuf::from(&exe_path);
        if !exe.exists() {
            return serde_json::json!({ "ok": false, "error": "executable not found" });
        }

        let working_dir = exe.parent().unwrap_or(&exe).to_string_lossy().to_string();
        let safe_exe = exe_path.replace('\'', "''");
        let safe_dir = working_dir.replace('\'', "''");
        let cmd_line = format!("set \"PATH={};%PATH%\" && \"{}\"", safe_dir, safe_exe);
        let ps_script = format!(
            "try {{ $p = Start-Process -FilePath 'cmd.exe' -ArgumentList @('/d','/s','/c', '{}') -WorkingDirectory '{}' -Verb RunAs -WindowStyle Hidden -PassThru -ErrorAction Stop; if ($p) {{ Write-Output \"STARTED:$($p.Id)\"; exit 0 }} else {{ Write-Error 'START-FAILED'; exit 1 }} }} catch {{ Write-Error $_.Exception.Message; exit 1 }}",
            cmd_line, safe_dir
        );

        let output = tokio::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-ExecutionPolicy", "Bypass", "-Command", &ps_script])
            .output()
            .await;

        match output {
            Ok(o) => {
                let stdout = String::from_utf8_lossy(&o.stdout);
                if let Some(cap) = regex::Regex::new(r"STARTED:(\d+)").unwrap().captures(&stdout) {
                    if let Ok(pid) = cap[1].parse::<u32>() {
                        register_running_game(&app, &appid, &exe_path, pid, game_name.as_deref(), show_game_name.unwrap_or(true));
                        return serde_json::json!({ "ok": true, "pid": pid });
                    }
                }
                // Fallback to regular launch
                game_exe_launch(app, appid, exe_path, game_name, show_game_name).await
            }
            Err(e) => game_exe_launch(app, appid, exe_path, game_name, show_game_name).await,
        }
    }
}

#[tauri::command]
pub async fn game_exe_running(app: AppHandle, appid: String) -> Value {
    let running = RUNNING_GAMES.lock().unwrap().get(&appid).cloned();
    match running {
        None => serde_json::json!({ "ok": true, "running": false }),
        Some(game) => {
            let alive = is_process_running(game.pid).await;
            if !alive {
                let mut games = RUNNING_GAMES.lock().unwrap();
                if let Some(ref appid_key) = game.appid { games.remove(appid_key); }
                if let Some(ref exe_key) = game.exe_path { games.remove(exe_key); }
                return serde_json::json!({ "ok": true, "running": false });
            }
            serde_json::json!({ "ok": true, "running": true, "pid": game.pid, "exePath": game.exe_path })
        }
    }
}

#[tauri::command]
pub async fn game_exe_quit(_app: AppHandle, appid: String) -> Value {
    let running = RUNNING_GAMES.lock().unwrap().get(&appid).cloned();
    match running {
        None => serde_json::json!({ "ok": true, "stopped": false }),
        Some(game) => {
            let stopped = kill_process_tree(game.pid).await;
            if stopped {
                let mut games = RUNNING_GAMES.lock().unwrap();
                if let Some(ref appid_key) = game.appid { games.remove(appid_key); }
                if let Some(ref exe_key) = game.exe_path { games.remove(exe_key); }
                // Clear game RPC activity if no more games running
                if games.is_empty() {
                    drop(games);
                    crate::rpc::clear_game_rpc_activity();
                }
            }
            serde_json::json!({ "ok": true, "stopped": stopped })
        }
    }
}

fn register_running_game(
    app: &AppHandle,
    appid: &str,
    exe_path: &str,
    pid: u32,
    game_name: Option<&str>,
    show_game_name: bool,
) {
    let started_at = chrono::Utc::now().timestamp_millis();
    let payload = RunningGame {
        appid: Some(appid.to_string()),
        exe_path: Some(exe_path.to_string()),
        game_name: game_name.map(|s| s.to_string()),
        pid,
        started_at,
    };
    {
        let mut games = RUNNING_GAMES.lock().unwrap();
        games.insert(appid.to_string(), payload.clone());
        games.insert(exe_path.to_string(), payload);
    }

    // Set Discord RPC activity
    let display_name = if show_game_name {
        game_name.unwrap_or(appid).to_string()
    } else {
        "A game".to_string()
    };
    let rpc_payload = serde_json::json!({
        "details": format!("Playing {}", display_name),
        "state": "Playing",
        "startTimestamp": started_at / 1000,
        "buttons": [
            { "label": "Open on web", "url": format!("https://union-crax.xyz/game/{}", appid) },
            { "label": "Download UC.D", "url": "https://union-crax.xyz/direct" }
        ]
    });
    crate::rpc::set_game_rpc_activity(&rpc_payload);
}

#[tauri::command]
pub async fn create_desktop_shortcut(app: AppHandle, game_name: String, exe_path: String) -> Value {
    let exe = PathBuf::from(&exe_path);
    if !exe.exists() {
        return serde_json::json!({ "ok": false, "error": "Executable not found" });
    }

    let desktop = match dirs::desktop_dir() {
        Some(d) => d,
        None => return serde_json::json!({ "ok": false, "error": "Could not find desktop directory" }),
    };

    #[cfg(target_os = "windows")]
    {
        let shortcut_name = format!("{} - UC.lnk", game_name);
        let shortcut_path = desktop.join(&shortcut_name);
        if shortcut_path.exists() {
            return serde_json::json!({ "ok": true, "existed": true });
        }

        let safe_exe = exe_path.replace('\'', "''");
        let safe_shortcut = shortcut_path.to_string_lossy().replace('\'', "''");
        let working_dir = exe.parent().unwrap_or(&exe).to_string_lossy().replace('\'', "''");
        let ps_script = format!(
            "$WshShell = New-Object -ComObject WScript.Shell; $Shortcut = $WshShell.CreateShortcut('{}'); $Shortcut.TargetPath = '{}'; $Shortcut.WorkingDirectory = '{}'; $Shortcut.Save()",
            safe_shortcut, safe_exe, working_dir
        );

        let output = tokio::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &ps_script])
            .output()
            .await;

        match output {
            Ok(o) if o.status.success() && shortcut_path.exists() => {
                serde_json::json!({ "ok": true })
            }
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr).to_string();
                serde_json::json!({ "ok": false, "error": err })
            }
            Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let safe_name = sanitize_desktop_filename(&game_name);
        let shortcut_name = format!("{} - UC.desktop", safe_name);
        let shortcut_path = desktop.join(&shortcut_name);
        if shortcut_path.exists() {
            return serde_json::json!({ "ok": true, "existed": true });
        }

        let working_dir = exe.parent().unwrap_or(&exe).to_string_lossy().to_string();
        let content = format!(
            "[Desktop Entry]\nType=Application\nName={}\nExec=\"{}\"\nPath={}\nTerminal=false\nCategories=Game;\n",
            game_name, exe_path, working_dir
        );
        match std::fs::write(&shortcut_path, content) {
            Ok(_) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&shortcut_path, std::fs::Permissions::from_mode(0o755));
                }
                serde_json::json!({ "ok": true })
            }
            Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
        }
    }
}

#[tauri::command]
pub async fn delete_desktop_shortcut(app: AppHandle, game_name: String) -> Value {
    let desktop = match dirs::desktop_dir() {
        Some(d) => d,
        None => return serde_json::json!({ "ok": false, "error": "Could not find desktop directory" }),
    };

    let shortcut_name = if cfg!(target_os = "windows") {
        format!("{} - UC.lnk", game_name)
    } else {
        format!("{} - UC.desktop", sanitize_desktop_filename(&game_name))
    };

    let shortcut_path = desktop.join(&shortcut_name);
    if !shortcut_path.exists() {
        return serde_json::json!({ "ok": true, "notFound": true });
    }

    match tokio::fs::remove_file(&shortcut_path).await {
        Ok(_) => serde_json::json!({ "ok": true }),
        Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
    }
}
