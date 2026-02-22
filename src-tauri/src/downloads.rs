/// Downloads module — mirrors the Electron main.cjs download management logic.
/// Uses reqwest for HTTP downloads with progress streaming, and 7-zip (via
/// bundled binary or system PATH) for archive extraction.
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::Context;
use futures_util::StreamExt;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter};
use tokio::io::AsyncWriteExt;

use crate::settings::read_settings;

// ── Constants ────────────────────────────────────────────────────────────────

const DOWNLOAD_DIR_NAME: &str = "UnionCrax.Direct";
const INSTALLING_DIR: &str = "installing";
const INSTALLED_DIR: &str = "installed";
const INSTALLED_MANIFEST: &str = "installed.json";
const INSTALLED_INDEX: &str = "installed-index.json";
const RESUME_BACKUP_EXT: &str = ".ucresume";
const PIXELDRAIN_DELAY_MS: u64 = 2000;

// ── Shared state ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveDownload {
    pub download_id: String,
    pub url: String,
    pub save_path: String,
    pub appid: Option<String>,
    pub game_name: Option<String>,
    pub filename: String,
    pub part_index: Option<u32>,
    pub part_total: Option<u32>,
    pub version_label: Option<String>,
    pub received_bytes: u64,
    pub total_bytes: u64,
    pub speed_bps: f64,
    pub cancelled: bool,
    pub paused: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadUpdate {
    #[serde(rename = "downloadId")]
    pub download_id: String,
    pub status: String,
    #[serde(rename = "receivedBytes", skip_serializing_if = "Option::is_none")]
    pub received_bytes: Option<u64>,
    #[serde(rename = "totalBytes", skip_serializing_if = "Option::is_none")]
    pub total_bytes: Option<u64>,
    #[serde(rename = "speedBps", skip_serializing_if = "Option::is_none")]
    pub speed_bps: Option<f64>,
    #[serde(rename = "etaSeconds", skip_serializing_if = "Option::is_none")]
    pub eta_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(rename = "savePath", skip_serializing_if = "Option::is_none")]
    pub save_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub appid: Option<String>,
    #[serde(rename = "gameName", skip_serializing_if = "Option::is_none")]
    pub game_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(rename = "partIndex", skip_serializing_if = "Option::is_none")]
    pub part_index: Option<u32>,
    #[serde(rename = "partTotal", skip_serializing_if = "Option::is_none")]
    pub part_total: Option<u32>,
}

static ACTIVE_DOWNLOADS: Lazy<Mutex<HashMap<String, ActiveDownload>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static GLOBAL_QUEUE: Lazy<Mutex<Vec<QueueEntry>>> = Lazy::new(|| Mutex::new(Vec::new()));
static CANCELLED_IDS: Lazy<Mutex<HashSet<String>>> = Lazy::new(|| Mutex::new(HashSet::new()));
static LAST_PIXELDRAIN_TIME: Lazy<Mutex<Instant>> =
    Lazy::new(|| Mutex::new(Instant::now() - Duration::from_secs(60)));

#[derive(Clone)]
struct QueueEntry {
    payload: DownloadPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadPayload {
    #[serde(rename = "downloadId")]
    pub download_id: String,
    pub url: String,
    pub filename: Option<String>,
    pub appid: Option<String>,
    #[serde(rename = "gameName")]
    pub game_name: Option<String>,
    #[serde(rename = "partIndex")]
    pub part_index: Option<u32>,
    #[serde(rename = "partTotal")]
    pub part_total: Option<u32>,
    #[serde(rename = "authHeader")]
    pub auth_header: Option<String>,
    #[serde(rename = "savePath")]
    pub save_path: Option<String>,
    #[serde(rename = "versionLabel")]
    pub version_label: Option<String>,
    #[serde(rename = "totalBytes")]
    pub total_bytes: Option<u64>,
    #[serde(rename = "resumeData")]
    pub resume_data: Option<ResumeData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeData {
    #[serde(rename = "urlChain")]
    pub url_chain: Option<Vec<String>>,
    #[serde(rename = "mimeType")]
    pub mime_type: Option<String>,
    pub etag: Option<String>,
    #[serde(rename = "lastModified")]
    pub last_modified: Option<String>,
    #[serde(rename = "startTime")]
    pub start_time: Option<u64>,
    pub offset: Option<u64>,
    #[serde(rename = "totalBytes")]
    pub total_bytes: Option<u64>,
    #[serde(rename = "savePath")]
    pub save_path: Option<String>,
}

// ── Path helpers ─────────────────────────────────────────────────────────────

pub fn get_default_download_root() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_string());
        PathBuf::from(format!("{}\\{}", drive, DOWNLOAD_DIR_NAME))
    }
    #[cfg(not(target_os = "windows"))]
    {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(DOWNLOAD_DIR_NAME)
    }
}

pub fn get_download_root(app: &AppHandle) -> PathBuf {
    let settings = read_settings(app);
    if let Some(Value::String(p)) = settings.get("downloadPath") {
        if !p.is_empty() {
            return normalize_download_root(p);
        }
    }
    get_default_download_root()
}

pub fn normalize_download_root(target: &str) -> PathBuf {
    let trimmed = target.trim().trim_end_matches(['/', '\\']);
    let p = PathBuf::from(trimmed);
    let base = p.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();

    let p = if base == INSTALLING_DIR || base == INSTALLED_DIR {
        p.parent().unwrap_or(&p).to_path_buf()
    } else {
        p
    };

    let base = p.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();

    if base != DOWNLOAD_DIR_NAME.to_lowercase() {
        p.join(DOWNLOAD_DIR_NAME)
    } else {
        p
    }
}

pub fn ensure_download_dir(app: &AppHandle) -> anyhow::Result<PathBuf> {
    let root = get_download_root(app);
    std::fs::create_dir_all(&root)?;
    std::fs::create_dir_all(root.join(INSTALLING_DIR))?;
    std::fs::create_dir_all(root.join(INSTALLED_DIR))?;
    Ok(root)
}

pub fn safe_folder_name(name: &str) -> String {
    if name.is_empty() {
        return "unioncrax-game".to_string();
    }
    let cleaned: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect::<String>()
        .trim_matches('_')
        .chars()
        .take(80)
        .collect();
    if cleaned.is_empty() {
        "unioncrax-game".to_string()
    } else {
        cleaned
    }
}

fn resolve_unique_path(dir: &Path, filename: &str) -> PathBuf {
    let candidate = dir.join(filename);
    if !candidate.exists() {
        return candidate;
    }
    let ext = Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let stem = Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(filename);
    for i in 1..=999 {
        let name = if ext.is_empty() {
            format!("{}-{}", stem, i)
        } else {
            format!("{}-{}.{}", stem, i, ext)
        };
        let c = dir.join(&name);
        if !c.exists() {
            return c;
        }
    }
    dir.join(format!("{}-{}", stem, chrono::Utc::now().timestamp_millis()))
}

fn is_multipart_path(path: &str) -> bool {
    regex::Regex::new(r"\.[0-9]{3}$").unwrap().is_match(path)
}

// ── Manifest helpers ─────────────────────────────────────────────────────────

fn read_json_file(path: &Path) -> Option<Value> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn write_json_file(path: &Path, data: &Value) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(data)?;
    std::fs::write(path, json)?;
    Ok(())
}

fn iterate_game_folders(root: &Path) -> Vec<(PathBuf, String, bool, Option<PathBuf>)> {
    let mut results = Vec::new();
    if !root.exists() {
        return results;
    }
    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return results,
    };
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let folder = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        results.push((folder.clone(), name.clone(), false, None));
        // Check versioned subdirs
        let versions_dir = folder.join("versions");
        if versions_dir.exists() {
            if let Ok(v_entries) = std::fs::read_dir(&versions_dir) {
                for v_entry in v_entries.flatten() {
                    if v_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        let v_folder = v_entry.path();
                        let v_name = v_entry.file_name().to_string_lossy().to_string();
                        results.push((v_folder, v_name, true, Some(folder.clone())));
                    }
                }
            }
        }
    }
    results
}

fn manifest_richness(m: &Value) -> i32 {
    let mut s = 0;
    if m.get("metadata").is_some() { s += 4; }
    if m.get("source").and_then(|v| v.as_str()).map(|s| s != "local").unwrap_or(false) { s += 3; }
    if m.get("name").is_some() { s += 1; }
    if m.get("description").is_some() { s += 1; }
    if m.get("image").is_some() { s += 1; }
    if m.get("release_date").is_some() { s += 1; }
    if m.get("size").is_some() { s += 1; }
    if m.get("genres").and_then(|v| v.as_array()).map(|a| !a.is_empty()).unwrap_or(false) { s += 1; }
    if m.get("developer").is_some() { s += 1; }
    s
}

fn list_manifests_from_root(root: &Path, allow_fallback: bool) -> Vec<Value> {
    let mut manifests: Vec<Value> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();

    for (folder, name, is_versioned, _) in iterate_game_folders(root) {
        let manifest_path = folder.join(INSTALLED_MANIFEST);
        if let Some(manifest) = read_json_file(&manifest_path) {
            if let Some(appid) = manifest.get("appid").and_then(|v| v.as_str()) {
                let appid = appid.to_string();
                if let Some(&idx) = seen.get(&appid) {
                    if manifest_richness(&manifest) > manifest_richness(&manifests[idx]) {
                        manifests[idx] = manifest;
                    }
                } else {
                    seen.insert(appid, manifests.len());
                    manifests.push(manifest);
                }
                continue;
            }
        }
        if allow_fallback && !is_versioned {
            let versions_dir = folder.join("versions");
            if versions_dir.exists() {
                continue;
            }
            if let Ok(files) = std::fs::read_dir(&folder) {
                let files: Vec<_> = files
                    .flatten()
                    .filter(|e| e.file_name().to_string_lossy() != INSTALLED_MANIFEST)
                    .collect();
                if !files.is_empty() && !seen.contains_key(&name) {
                    let file_names: Vec<Value> = files
                        .iter()
                        .map(|f| serde_json::json!({ "name": f.file_name().to_string_lossy() }))
                        .collect();
                    seen.insert(name.clone(), manifests.len());
                    manifests.push(serde_json::json!({
                        "appid": name,
                        "name": name,
                        "files": file_names
                    }));
                }
            }
        }
    }
    manifests
}

fn update_installed_manifest_bulk(
    installed_folder: &Path,
    metadata: &Value,
    file_entries: &[Value],
) {
    let manifest_path = installed_folder.join(INSTALLED_MANIFEST);
    let mut manifest = read_json_file(&manifest_path).unwrap_or_else(|| serde_json::json!({}));

    if let Some(appid) = metadata.get("appid") {
        manifest["appid"] = appid.clone();
    }
    if let Some(name) = metadata.get("name") {
        manifest["name"] = name.clone();
    }
    manifest["metadata"] = metadata.clone();

    // Compute metadata hash
    if let Ok(hash) = compute_object_hash(metadata) {
        manifest["metadataHash"] = Value::String(hash);
    }

    let files = manifest["files"].as_array_mut().map(|a| {
        let existing_paths: HashSet<String> = a
            .iter()
            .filter_map(|f| f.get("path").and_then(|p| p.as_str()).map(|s| s.to_string()))
            .collect();
        (existing_paths, a as *mut Vec<Value>)
    });

    if let Some((mut existing_paths, files_ptr)) = files {
        let files_arr = unsafe { &mut *files_ptr };
        for entry in file_entries {
            if let Some(path) = entry.get("path").and_then(|p| p.as_str()) {
                if !existing_paths.contains(path) {
                    existing_paths.insert(path.to_string());
                    files_arr.push(entry.clone());
                }
            }
        }
    } else {
        manifest["files"] = Value::Array(file_entries.to_vec());
    }

    if manifest.get("installedAt").is_none() || manifest["installedAt"].is_null() {
        manifest["installedAt"] = Value::Number(
            chrono::Utc::now().timestamp_millis().into(),
        );
    }

    let _ = write_json_file(&manifest_path, &manifest);

    // Update root installed index
    if let Some(parent) = installed_folder.parent() {
        update_installed_index(parent);
    }
}

fn update_installed_index(installed_root: &Path) {
    if !installed_root.exists() {
        return;
    }
    let mut index = Vec::new();
    let mut seen_appids = HashSet::new();

    for (folder, name, _, _) in iterate_game_folders(installed_root) {
        let manifest_path = folder.join(INSTALLED_MANIFEST);
        if let Some(manifest) = read_json_file(&manifest_path) {
            if let Some(appid) = manifest.get("appid").and_then(|v| v.as_str()) {
                if !seen_appids.contains(appid) {
                    seen_appids.insert(appid.to_string());
                    let rel = folder.strip_prefix(installed_root)
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|_| name.clone());
                    index.push(serde_json::json!({
                        "appid": appid,
                        "name": manifest.get("name").cloned().unwrap_or(Value::String(name)),
                        "folder": rel,
                        "manifestPath": manifest_path.to_string_lossy()
                    }));
                }
            }
        }
    }

    let index_path = installed_root.join(INSTALLED_INDEX);
    let _ = write_json_file(&index_path, &Value::Array(index));
}

fn compute_object_hash(obj: &Value) -> anyhow::Result<String> {
    let raw = serde_json::to_string(obj)?;
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    Ok(hex::encode(hasher.finalize()))
}

async fn compute_file_checksum(path: &Path) -> Option<String> {
    let data = tokio::fs::read(path).await.ok()?;
    let mut hasher = Sha256::new();
    hasher.update(&data);
    Some(hex::encode(hasher.finalize()))
}

#[allow(dead_code)]
fn find_installed_folder_by_appid(app: &AppHandle, appid: &str) -> Option<PathBuf> {
    for root in list_download_roots(app) {
        let installed_root = root.join(INSTALLED_DIR);
        for (folder, name, _, _) in iterate_game_folders(&installed_root) {
            let manifest_path = folder.join(INSTALLED_MANIFEST);
            if let Some(manifest) = read_json_file(&manifest_path) {
                if manifest.get("appid").and_then(|v| v.as_str()) == Some(appid) {
                    return Some(folder);
                }
            }
            if name == appid {
                return Some(folder);
            }
        }
    }
    None
}

fn find_installing_folder_by_appid(app: &AppHandle, appid: &str) -> Option<PathBuf> {
    for root in list_download_roots(app) {
        let installing_root = root.join(INSTALLING_DIR);
        for (folder, name, _, _) in iterate_game_folders(&installing_root) {
            let manifest_path = folder.join(INSTALLED_MANIFEST);
            if let Some(manifest) = read_json_file(&manifest_path) {
                if manifest.get("appid").and_then(|v| v.as_str()) == Some(appid) {
                    return Some(folder);
                }
            }
            if name == appid {
                return Some(folder);
            }
        }
    }
    None
}

pub fn list_download_roots(app: &AppHandle) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let settings = read_settings(app);
    if let Some(Value::String(p)) = settings.get("downloadPath") {
        if !p.is_empty() {
            roots.push(normalize_download_root(p));
        }
    }
    let default_root = get_download_root(app);
    if !roots.contains(&default_root) {
        roots.push(default_root);
    }
    // Add disk roots on Windows
    #[cfg(target_os = "windows")]
    {
        for disk in list_disks_inner() {
            let r = PathBuf::from(&disk.path).join(DOWNLOAD_DIR_NAME);
            if !roots.contains(&r) {
                roots.push(r);
            }
        }
    }
    roots.into_iter().filter(|r| r.exists()).collect()
}

fn send_download_update(app: &AppHandle, update: &DownloadUpdate) {
    let _ = app.emit("uc:download-update", update);
}

// ── Disk listing ─────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone)]
pub struct DiskInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    #[serde(rename = "totalBytes")]
    pub total_bytes: u64,
    #[serde(rename = "freeBytes")]
    pub free_bytes: u64,
}

#[cfg(target_os = "windows")]
fn list_disks_inner() -> Vec<DiskInfo> {
    let mut disks = Vec::new();
    for letter in b'A'..=b'Z' {
        let root = format!("{}:\\", letter as char);
        let path = PathBuf::from(&root);
        if !path.exists() {
            continue;
        }
        // Use statvfs equivalent via winapi
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = std::ffi::OsStr::new(&root)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut free_bytes_available: u64 = 0;
        let mut total_bytes: u64 = 0;
        let mut total_free_bytes: u64 = 0;
        unsafe {
            winapi::um::fileapi::GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut free_bytes_available as *mut u64 as *mut _,
                &mut total_bytes as *mut u64 as *mut _,
                &mut total_free_bytes as *mut u64 as *mut _,
            );
        }
        disks.push(DiskInfo {
            id: (letter as char).to_string(),
            name: format!("{}:", letter as char),
            path: root,
            total_bytes,
            free_bytes: free_bytes_available,
        });
    }
    disks
}

#[cfg(not(target_os = "windows"))]
fn list_disks_inner() -> Vec<DiskInfo> {
    let mut disks = Vec::new();
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let candidates = vec![home.clone(), PathBuf::from("/home"), PathBuf::from("/mnt"), PathBuf::from("/media")];
    for root in candidates {
        if !root.exists() {
            continue;
        }
        let id = if root == home { "home".to_string() } else { root.to_string_lossy().replace('/', "_") };
        let name = if root == home { "Home".to_string() } else { root.to_string_lossy().to_string() };
        disks.push(DiskInfo {
            id,
            name,
            path: root.to_string_lossy().to_string(),
            total_bytes: 0,
            free_bytes: 0,
        });
    }
    disks
}

// ── 7-zip extraction ─────────────────────────────────────────────────────────

fn resolve_7zip_binary() -> Option<String> {
    // Check bundled binary first
    #[cfg(target_os = "windows")]
    let candidates = vec!["7z.exe", "7za.exe", "7z"];
    #[cfg(not(target_os = "windows"))]
    let candidates = vec!["7z", "7za"];

    for candidate in &candidates {
        if which::which(candidate).is_ok() {
            return Some(candidate.to_string());
        }
    }
    None
}

async fn run_7z_extract(
    _app: &AppHandle,
    archive_path: &Path,
    dest_dir: &Path,
    _download_id: &str,
    _total_bytes: u64,
    _appid: Option<&str>,
) -> anyhow::Result<Vec<PathBuf>> {
    let cmd = resolve_7zip_binary()
        .context("7zip binary not found. Please install p7zip (7z) on this system.")?;

    // Snapshot files before extraction
    let before: HashSet<PathBuf> = snapshot_files(dest_dir);

    let output = tokio::process::Command::new(&cmd)
        .args(["x", &archive_path.to_string_lossy(), &format!("-o{}", dest_dir.to_string_lossy()), "-y"])
        .output()
        .await
        .context("Failed to spawn 7zip")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        anyhow::bail!("7zip failed: {}", if stderr.is_empty() { stdout } else { stderr });
    }

    let after: HashSet<PathBuf> = snapshot_files(dest_dir);
    let extracted: Vec<PathBuf> = after.difference(&before).cloned().collect();
    Ok(extracted)
}

fn snapshot_files(root: &Path) -> HashSet<PathBuf> {
    let mut files = HashSet::new();
    if !root.exists() {
        return files;
    }
    let mut pending = vec![root.to_path_buf()];
    while let Some(cur) = pending.pop() {
        if let Ok(entries) = std::fs::read_dir(&cur) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    pending.push(path);
                } else if path.is_file() {
                    files.insert(path);
                }
            }
        }
    }
    files
}

fn migrate_installing_extras(installing_root: &Path, installed_root: &Path, skip_names: &HashSet<String>) {
    if !installing_root.exists() {
        return;
    }
    if let Ok(items) = std::fs::read_dir(installing_root) {
        for item in items.flatten() {
            let name = item.file_name().to_string_lossy().to_string();
            if skip_names.contains(&name) || name == INSTALLED_MANIFEST || name == INSTALLED_INDEX {
                continue;
            }
            let src = item.path();
            let dest = resolve_unique_path(installed_root, &name);
            let _ = std::fs::rename(&src, &dest).or_else(|_| {
                std::fs::copy(&src, &dest).map(|_| ()).and_then(|_| std::fs::remove_file(&src))
            });
        }
    }
}

// ── Download execution ────────────────────────────────────────────────────────

async fn download_file(
    app: AppHandle,
    payload: DownloadPayload,
) {
    let download_id = payload.download_id.clone();
    let url = payload.url.clone();

    // Check if cancelled
    if CANCELLED_IDS.lock().unwrap().contains(&download_id) {
        return;
    }

    let download_root = match ensure_download_dir(&app) {
        Ok(r) => r,
        Err(e) => {
            send_download_update(&app, &DownloadUpdate {
                download_id: download_id.clone(),
                status: "failed".to_string(),
                error: Some(e.to_string()),
                ..Default::default()
            });
            return;
        }
    };

    let game_name = payload.game_name.as_deref().unwrap_or("");
    let appid = payload.appid.as_deref().unwrap_or(&download_id);
    let folder_name = safe_folder_name(if !game_name.is_empty() { game_name } else { appid });
    let version_slug = payload.version_label.as_deref().map(safe_folder_name);
    let actual_folder = if let Some(ref vs) = version_slug {
        PathBuf::from(&folder_name).join("versions").join(vs)
    } else {
        PathBuf::from(&folder_name)
    };

    let installing_root = download_root.join(INSTALLING_DIR).join(&actual_folder);
    let installed_root = download_root.join(INSTALLED_DIR).join(&actual_folder);
    std::fs::create_dir_all(&installing_root).unwrap_or_default();
    std::fs::create_dir_all(&installed_root).unwrap_or_default();

    // Determine filename
    let filename = payload.filename.clone().unwrap_or_else(|| {
        url.split('/').last().unwrap_or("download").to_string()
    });
    let save_path = payload.save_path.clone()
        .map(PathBuf::from)
        .unwrap_or_else(|| installing_root.join(&filename));

    // Register active download
    {
        let mut active = ACTIVE_DOWNLOADS.lock().unwrap();
        active.insert(download_id.clone(), ActiveDownload {
            download_id: download_id.clone(),
            url: url.clone(),
            save_path: save_path.to_string_lossy().to_string(),
            appid: payload.appid.clone(),
            game_name: payload.game_name.clone(),
            filename: filename.clone(),
            part_index: payload.part_index,
            part_total: payload.part_total,
            version_label: payload.version_label.clone(),
            received_bytes: 0,
            total_bytes: 0,
            speed_bps: 0.0,
            cancelled: false,
            paused: false,
        });
    }

    // Build HTTP request
    let mut req_builder = reqwest::Client::new().get(&url);
    if let Some(ref auth) = payload.auth_header {
        req_builder = req_builder.header("Authorization", auth);
    }
    if url.contains("pixeldrain.com") {
        req_builder = req_builder
            .header("Referer", "https://pixeldrain.com/")
            .header("Origin", "https://pixeldrain.com");
    }
    // Resume support
    let resume_offset = if save_path.exists() {
        save_path.metadata().map(|m| m.len()).unwrap_or(0)
    } else {
        0
    };
    if resume_offset > 0 {
        req_builder = req_builder.header("Range", format!("bytes={}-", resume_offset));
    }

    let response = match req_builder.send().await {
        Ok(r) => r,
        Err(e) => {
            ACTIVE_DOWNLOADS.lock().unwrap().remove(&download_id);
            send_download_update(&app, &DownloadUpdate {
                download_id: download_id.clone(),
                status: "failed".to_string(),
                error: Some(e.to_string()),
                appid: payload.appid.clone(),
                game_name: payload.game_name.clone(),
                ..Default::default()
            });
            return;
        }
    };

    let total_bytes = response.content_length().unwrap_or(0) + resume_offset;
    let status_code = response.status();

    if !status_code.is_success() && status_code.as_u16() != 206 {
        ACTIVE_DOWNLOADS.lock().unwrap().remove(&download_id);
        send_download_update(&app, &DownloadUpdate {
            download_id: download_id.clone(),
            status: "failed".to_string(),
            error: Some(format!("HTTP {}", status_code)),
            appid: payload.appid.clone(),
            ..Default::default()
        });
        return;
    }

    // Send initial update
    send_download_update(&app, &DownloadUpdate {
        download_id: download_id.clone(),
        status: "downloading".to_string(),
        received_bytes: Some(resume_offset),
        total_bytes: Some(total_bytes),
        speed_bps: Some(0.0),
        eta_seconds: None,
        filename: Some(filename.clone()),
        save_path: Some(save_path.to_string_lossy().to_string()),
        appid: payload.appid.clone(),
        game_name: payload.game_name.clone(),
        url: Some(url.clone()),
        part_index: payload.part_index,
        part_total: payload.part_total,
        ..Default::default()
    });

    // Stream to file
    let file = match tokio::fs::OpenOptions::new()
        .create(true)
        .append(resume_offset > 0)
        .write(true)
        .open(&save_path)
        .await
    {
        Ok(f) => f,
        Err(e) => {
            ACTIVE_DOWNLOADS.lock().unwrap().remove(&download_id);
            send_download_update(&app, &DownloadUpdate {
                download_id: download_id.clone(),
                status: "failed".to_string(),
                error: Some(e.to_string()),
                ..Default::default()
            });
            return;
        }
    };

    let mut writer = tokio::io::BufWriter::new(file);
    let mut stream = response.bytes_stream();
    let mut received = resume_offset;
    let mut last_speed_update = Instant::now();
    let mut last_bytes = resume_offset;
    let mut speed_bps: f64 = 0.0;
    let mut last_emit = Instant::now();

    while let Some(chunk) = stream.next().await {
        // Check cancellation
        if CANCELLED_IDS.lock().unwrap().contains(&download_id) {
            let _ = writer.flush().await;
            ACTIVE_DOWNLOADS.lock().unwrap().remove(&download_id);
            send_download_update(&app, &DownloadUpdate {
                download_id: download_id.clone(),
                status: "cancelled".to_string(),
                appid: payload.appid.clone(),
                ..Default::default()
            });
            return;
        }

        // Check pause
        loop {
            let paused = ACTIVE_DOWNLOADS.lock().unwrap()
                .get(&download_id)
                .map(|d| d.paused)
                .unwrap_or(false);
            if !paused { break; }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        match chunk {
            Ok(data) => {
                if let Err(e) = writer.write_all(&data).await {
                    ACTIVE_DOWNLOADS.lock().unwrap().remove(&download_id);
                    send_download_update(&app, &DownloadUpdate {
                        download_id: download_id.clone(),
                        status: "failed".to_string(),
                        error: Some(e.to_string()),
                        ..Default::default()
                    });
                    return;
                }
                received += data.len() as u64;

                // Update speed
                let now = Instant::now();
                let delta_time = now.duration_since(last_speed_update).as_secs_f64();
                if delta_time >= 0.5 {
                    let delta_bytes = received.saturating_sub(last_bytes) as f64;
                    let instant_speed = delta_bytes / delta_time;
                    speed_bps = if speed_bps > 0.0 {
                        speed_bps * 0.7 + instant_speed * 0.3
                    } else {
                        instant_speed
                    };
                    last_bytes = received;
                    last_speed_update = now;
                }

                // Update active download state
                if let Some(entry) = ACTIVE_DOWNLOADS.lock().unwrap().get_mut(&download_id) {
                    entry.received_bytes = received;
                    entry.total_bytes = total_bytes;
                    entry.speed_bps = speed_bps;
                }

                // Throttle UI updates to ~3/sec
                if last_emit.elapsed() >= Duration::from_millis(333) {
                    last_emit = Instant::now();
                    let remaining = if total_bytes > 0 { total_bytes.saturating_sub(received) } else { 0 };
                    let eta = if speed_bps > 0.0 && remaining > 0 {
                        Some(remaining as f64 / speed_bps)
                    } else {
                        None
                    };
                    send_download_update(&app, &DownloadUpdate {
                        download_id: download_id.clone(),
                        status: "downloading".to_string(),
                        received_bytes: Some(received),
                        total_bytes: Some(total_bytes),
                        speed_bps: Some(speed_bps),
                        eta_seconds: eta,
                        filename: Some(filename.clone()),
                        save_path: Some(save_path.to_string_lossy().to_string()),
                        appid: payload.appid.clone(),
                        game_name: payload.game_name.clone(),
                        url: Some(url.clone()),
                        part_index: payload.part_index,
                        part_total: payload.part_total,
                        ..Default::default()
                    });
                }
            }
            Err(e) => {
                let _ = writer.flush().await;
                ACTIVE_DOWNLOADS.lock().unwrap().remove(&download_id);
                send_download_update(&app, &DownloadUpdate {
                    download_id: download_id.clone(),
                    status: "failed".to_string(),
                    error: Some(e.to_string()),
                    appid: payload.appid.clone(),
                    ..Default::default()
                });
                return;
            }
        }
    }

    let _ = writer.flush().await;
    ACTIVE_DOWNLOADS.lock().unwrap().remove(&download_id);

    // Post-download: extract if archive
    let final_path = save_path.clone();
    let ext = final_path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let is_archive = matches!(ext.as_str(), "zip" | "7z" | "rar" | "tar" | "gz" | "tgz")
        || is_multipart_path(&final_path.to_string_lossy());

    let metadata = serde_json::json!({
        "appid": payload.appid,
        "name": payload.game_name.as_deref().unwrap_or(appid),
    });

    if is_archive && final_path.exists() {
        send_download_update(&app, &DownloadUpdate {
            download_id: download_id.clone(),
            status: "extracting".to_string(),
            received_bytes: Some(0),
            total_bytes: Some(total_bytes),
            speed_bps: Some(0.0),
            filename: Some(filename.clone()),
            save_path: Some(final_path.to_string_lossy().to_string()),
            appid: payload.appid.clone(),
            ..Default::default()
        });

        match run_7z_extract(&app, &final_path, &installed_root, &download_id, total_bytes, payload.appid.as_deref()).await {
            Ok(extracted_files) => {
                let file_entries: Vec<Value> = extracted_files.iter().map(|f| {
                    let size = f.metadata().map(|m| m.len()).unwrap_or(0);
                    serde_json::json!({
                        "path": f.to_string_lossy(),
                        "name": f.file_name().and_then(|n| n.to_str()).unwrap_or(""),
                        "size": size,
                        "checksum": null,
                        "addedAt": chrono::Utc::now().timestamp_millis()
                    })
                }).collect();

                update_installed_manifest_bulk(&installed_root, &metadata, &file_entries);

                // Clean up archive
                let _ = std::fs::remove_file(&final_path);

                // Migrate extras
                let mut skip = HashSet::new();
                skip.insert(filename.clone());
                migrate_installing_extras(&installing_root, &installed_root, &skip);

                // Clean up installing folder
                let _ = std::fs::remove_dir_all(&installing_root);

                send_download_update(&app, &DownloadUpdate {
                    download_id: download_id.clone(),
                    status: "extracted".to_string(),
                    appid: payload.appid.clone(),
                    ..Default::default()
                });
                send_download_update(&app, &DownloadUpdate {
                    download_id: download_id.clone(),
                    status: "completed".to_string(),
                    received_bytes: Some(total_bytes),
                    total_bytes: Some(total_bytes),
                    speed_bps: Some(0.0),
                    eta_seconds: Some(0.0),
                    filename: Some(filename.clone()),
                    appid: payload.appid.clone(),
                    game_name: payload.game_name.clone(),
                    url: Some(url.clone()),
                    part_index: payload.part_index,
                    part_total: payload.part_total,
                    ..Default::default()
                });
            }
            Err(e) => {
                send_download_update(&app, &DownloadUpdate {
                    download_id: download_id.clone(),
                    status: "extract_failed".to_string(),
                    error: Some(e.to_string()),
                    save_path: Some(final_path.to_string_lossy().to_string()),
                    appid: payload.appid.clone(),
                    ..Default::default()
                });
            }
        }
    } else {
        // Not an archive — move to installed folder
        let target = resolve_unique_path(&installed_root, &filename);
        let _ = std::fs::rename(&final_path, &target).or_else(|_| {
            std::fs::copy(&final_path, &target).map(|_| ()).and_then(|_| std::fs::remove_file(&final_path))
        });

        let checksum = compute_file_checksum(&target).await;
        let size = target.metadata().map(|m| m.len()).unwrap_or(0);
        let file_entry = serde_json::json!({
            "path": target.to_string_lossy(),
            "name": target.file_name().and_then(|n| n.to_str()).unwrap_or(""),
            "size": size,
            "checksum": checksum,
            "addedAt": chrono::Utc::now().timestamp_millis()
        });
        update_installed_manifest_bulk(&installed_root, &metadata, &[file_entry]);

        let mut skip = HashSet::new();
        skip.insert(filename.clone());
        migrate_installing_extras(&installing_root, &installed_root, &skip);
        let _ = std::fs::remove_dir_all(&installing_root);

        send_download_update(&app, &DownloadUpdate {
            download_id: download_id.clone(),
            status: "completed".to_string(),
            received_bytes: Some(total_bytes),
            total_bytes: Some(total_bytes),
            speed_bps: Some(0.0),
            eta_seconds: Some(0.0),
            filename: Some(filename.clone()),
            save_path: Some(target.to_string_lossy().to_string()),
            appid: payload.appid.clone(),
            game_name: payload.game_name.clone(),
            url: Some(url.clone()),
            part_index: payload.part_index,
            part_total: payload.part_total,
            ..Default::default()
        });
    }

    // Start next queued download
    start_next_queued(&app);
}

fn start_next_queued(app: &AppHandle) {
    let next = {
        let mut queue = GLOBAL_QUEUE.lock().unwrap();
        if queue.is_empty() { return; }
        let active = ACTIVE_DOWNLOADS.lock().unwrap();
        if !active.is_empty() { return; }
        queue.remove(0)
    };
    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        download_file(app_clone, next.payload).await;
    });
}

// ── Default impl ─────────────────────────────────────────────────────────────

impl Default for DownloadUpdate {
    fn default() -> Self {
        Self {
            download_id: String::new(),
            status: String::new(),
            received_bytes: None,
            total_bytes: None,
            speed_bps: None,
            eta_seconds: None,
            filename: None,
            save_path: None,
            appid: None,
            game_name: None,
            url: None,
            error: None,
            part_index: None,
            part_total: None,
        }
    }
}

// ── Tauri commands ────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn download_start(app: AppHandle, payload: DownloadPayload) -> Value {
    let download_id = payload.download_id.clone();

    // Check if already active/queued
    {
        let active = ACTIVE_DOWNLOADS.lock().unwrap();
        if active.contains_key(&download_id) {
            return serde_json::json!({ "ok": true, "already": true, "state": "active" });
        }
    }
    {
        let queue = GLOBAL_QUEUE.lock().unwrap();
        if queue.iter().any(|e| e.payload.download_id == download_id) {
            return serde_json::json!({ "ok": true, "already": true, "state": "queued" });
        }
    }

    // Pixeldrain delay for unauthenticated downloads
    let is_pixeldrain = payload.url.contains("pixeldrain.com");
    let has_auth = payload.auth_header.is_some();
    if is_pixeldrain && !has_auth {
        let elapsed = {
            let last = LAST_PIXELDRAIN_TIME.lock().unwrap();
            last.elapsed()
        };
        if elapsed < Duration::from_millis(PIXELDRAIN_DELAY_MS) {
            let delay = Duration::from_millis(PIXELDRAIN_DELAY_MS) - elapsed;
            tokio::time::sleep(delay).await;
        }
        *LAST_PIXELDRAIN_TIME.lock().unwrap() = Instant::now();
    }

    // Queue or start immediately
    let has_active = !ACTIVE_DOWNLOADS.lock().unwrap().is_empty();
    let has_queued = !GLOBAL_QUEUE.lock().unwrap().is_empty();

    if has_active || has_queued {
        GLOBAL_QUEUE.lock().unwrap().push(QueueEntry { payload });
        return serde_json::json!({ "ok": true, "queued": true });
    }

    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        download_file(app_clone, payload).await;
    });

    serde_json::json!({ "ok": true })
}

#[tauri::command]
pub fn download_cancel(_app: AppHandle, download_id: String) -> Value {
    CANCELLED_IDS.lock().unwrap().insert(download_id.clone());

    // Mark active download as cancelled
    if let Some(entry) = ACTIVE_DOWNLOADS.lock().unwrap().get_mut(&download_id) {
        entry.cancelled = true;
        return serde_json::json!({ "ok": true });
    }

    // Remove from queue
    let mut queue = GLOBAL_QUEUE.lock().unwrap();
    let before = queue.len();
    queue.retain(|e| e.payload.download_id != download_id);
    if queue.len() < before {
        return serde_json::json!({ "ok": true });
    }

    serde_json::json!({ "ok": false })
}

#[tauri::command]
pub fn download_pause(app: AppHandle, download_id: String) -> Value {
    if let Some(entry) = ACTIVE_DOWNLOADS.lock().unwrap().get_mut(&download_id) {
        entry.paused = true;
        let update = DownloadUpdate {
            download_id: download_id.clone(),
            status: "paused".to_string(),
            received_bytes: Some(entry.received_bytes),
            total_bytes: Some(entry.total_bytes),
            speed_bps: Some(0.0),
            filename: Some(entry.filename.clone()),
            save_path: Some(entry.save_path.clone()),
            appid: entry.appid.clone(),
            game_name: entry.game_name.clone(),
            url: Some(entry.url.clone()),
            part_index: entry.part_index,
            part_total: entry.part_total,
            ..Default::default()
        };
        send_download_update(&app, &update);
        return serde_json::json!({ "ok": true });
    }
    serde_json::json!({ "ok": false })
}

#[tauri::command]
pub fn download_resume(app: AppHandle, download_id: String) -> Value {
    if let Some(entry) = ACTIVE_DOWNLOADS.lock().unwrap().get_mut(&download_id) {
        entry.paused = false;
        let update = DownloadUpdate {
            download_id: download_id.clone(),
            status: "downloading".to_string(),
            received_bytes: Some(entry.received_bytes),
            total_bytes: Some(entry.total_bytes),
            speed_bps: Some(0.0),
            filename: Some(entry.filename.clone()),
            save_path: Some(entry.save_path.clone()),
            appid: entry.appid.clone(),
            game_name: entry.game_name.clone(),
            url: Some(entry.url.clone()),
            part_index: entry.part_index,
            part_total: entry.part_total,
            ..Default::default()
        };
        send_download_update(&app, &update);
        return serde_json::json!({ "ok": true });
    }
    serde_json::json!({ "ok": false })
}

#[tauri::command]
pub async fn download_resume_interrupted(app: AppHandle, payload: DownloadPayload) -> Value {
    // Check if partial file exists
    let save_path = match &payload.save_path {
        Some(p) => PathBuf::from(p),
        None => return serde_json::json!({ "ok": false, "error": "missing-save-path" }),
    };

    // Check for .ucresume backup
    let backup = PathBuf::from(format!("{}{}", save_path.to_string_lossy(), RESUME_BACKUP_EXT));
    if !save_path.exists() && backup.exists() {
        let _ = std::fs::rename(&backup, &save_path);
    }

    if !save_path.exists() {
        return serde_json::json!({ "ok": false, "error": "missing-file" });
    }

    let actual_offset = save_path.metadata().map(|m| m.len()).unwrap_or(0);
    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        download_file(app_clone, payload).await;
    });

    serde_json::json!({ "ok": true, "actualOffset": actual_offset })
}

#[tauri::command]
pub async fn download_resume_with_fresh_url(app: AppHandle, payload: DownloadPayload) -> Value {
    let save_path = match &payload.save_path {
        Some(p) => PathBuf::from(p),
        None => return serde_json::json!({ "ok": false, "error": "missing-url" }),
    };

    let backup = PathBuf::from(format!("{}{}", save_path.to_string_lossy(), RESUME_BACKUP_EXT));
    if !save_path.exists() && backup.exists() {
        let _ = std::fs::rename(&backup, &save_path);
    }

    if !save_path.exists() {
        return serde_json::json!({ "ok": false, "error": "missing-file" });
    }

    let actual_offset = save_path.metadata().map(|m| m.len()).unwrap_or(0);
    if actual_offset == 0 {
        return serde_json::json!({ "ok": false, "error": "empty-file" });
    }

    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        download_file(app_clone, payload).await;
    });

    serde_json::json!({ "ok": true, "actualOffset": actual_offset })
}

#[tauri::command]
pub async fn download_show(app: AppHandle, target_path: String) -> Value {
    use tauri_plugin_opener::OpenerExt;
    let path = PathBuf::from(&target_path);
    let folder = path.parent().unwrap_or(&path);
    let _ = app.opener().open_path(folder.to_string_lossy().as_ref(), None::<&str>);
    serde_json::json!({ "ok": true })
}

#[tauri::command]
pub async fn download_open(app: AppHandle, target_path: String) -> Value {
    use tauri_plugin_opener::OpenerExt;
    let _ = app.opener().open_path(&target_path, None::<&str>);
    serde_json::json!({ "ok": true })
}

#[tauri::command]
pub fn disk_list() -> Vec<DiskInfo> {
    list_disks_inner()
}

#[tauri::command]
pub fn download_path_get(app: AppHandle) -> Value {
    let path = ensure_download_dir(&app)
        .unwrap_or_else(|_| get_download_root(&app));
    serde_json::json!({ "path": path.to_string_lossy() })
}

#[tauri::command]
pub fn download_path_set(app: AppHandle, target_path: String) -> Value {
    let normalized = normalize_download_root(&target_path);
    let mut settings = read_settings(&app);
    settings.insert("downloadPath".to_string(), Value::String(normalized.to_string_lossy().to_string()));
    crate::settings::write_settings(&app, &settings);
    let resolved = ensure_download_dir(&app).unwrap_or(normalized);
    serde_json::json!({ "ok": true, "path": resolved.to_string_lossy() })
}

#[tauri::command]
pub async fn download_path_pick(app: AppHandle) -> Value {
    use tauri_plugin_dialog::DialogExt;
    let path = app.dialog()
        .file()
        .set_title("Select Download Folder")
        .blocking_pick_folder();

    match path {
        Some(p) => {
            let path_str = p.as_path()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| p.to_string());
            let normalized = normalize_download_root(&path_str);
            let mut settings = read_settings(&app);
            settings.insert("downloadPath".to_string(), Value::String(normalized.to_string_lossy().to_string()));
            crate::settings::write_settings(&app, &settings);
            let resolved = ensure_download_dir(&app).unwrap_or(normalized);
            serde_json::json!({ "ok": true, "path": resolved.to_string_lossy() })
        }
        None => serde_json::json!({ "ok": false }),
    }
}

#[tauri::command]
pub async fn download_usage(app: AppHandle, target_path: Option<String>) -> Value {
    let path = target_path
        .map(PathBuf::from)
        .unwrap_or_else(|| ensure_download_dir(&app).unwrap_or_else(|_| get_download_root(&app)));

    let size = get_directory_size(&path).await;
    serde_json::json!({ "ok": true, "sizeBytes": size, "path": path.to_string_lossy() })
}

async fn get_directory_size(path: &Path) -> u64 {
    let mut total = 0u64;
    let mut pending = vec![path.to_path_buf()];
    while let Some(cur) = pending.pop() {
        if let Ok(mut entries) = tokio::fs::read_dir(&cur).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let p = entry.path();
                if p.is_dir() {
                    pending.push(p);
                } else if let Ok(meta) = tokio::fs::metadata(&p).await {
                    total += meta.len();
                }
            }
        }
    }
    total
}

#[tauri::command]
pub fn download_cache_clear(app: AppHandle) -> Value {
    let active = ACTIVE_DOWNLOADS.lock().unwrap();
    if !active.is_empty() {
        return serde_json::json!({ "ok": false, "error": "downloads-active" });
    }
    drop(active);

    let root = match ensure_download_dir(&app) {
        Ok(r) => r,
        Err(e) => return serde_json::json!({ "ok": false, "error": e.to_string() }),
    };
    let installing_root = root.join(INSTALLING_DIR);
    if installing_root.exists() {
        let _ = std::fs::remove_dir_all(&installing_root);
    }
    let _ = std::fs::create_dir_all(&installing_root);
    serde_json::json!({ "ok": true })
}

#[tauri::command]
pub async fn installed_save(app: AppHandle, appid: String, metadata: Value) -> Value {
    let download_root = match ensure_download_dir(&app) {
        Ok(r) => r,
        Err(e) => return serde_json::json!({ "ok": false, "error": e.to_string() }),
    };

    let folder_name = safe_folder_name(
        metadata.get("name").and_then(|v| v.as_str())
            .or_else(|| metadata.get("gameName").and_then(|v| v.as_str()))
            .unwrap_or(&appid)
    );
    let version_label = metadata.get("downloadedVersion").and_then(|v| v.as_str());
    let version_slug = version_label.map(safe_folder_name);
    let actual_folder = if let Some(ref vs) = version_slug {
        PathBuf::from(&folder_name).join("versions").join(vs)
    } else {
        PathBuf::from(&folder_name)
    };

    let installing_root = download_root.join(INSTALLING_DIR).join(&actual_folder);
    std::fs::create_dir_all(&installing_root).unwrap_or_default();

    let manifest_path = installing_root.join(INSTALLED_MANIFEST);
    let mut manifest = read_json_file(&manifest_path).unwrap_or_else(|| serde_json::json!({}));
    manifest["appid"] = Value::String(appid.clone());
    manifest["name"] = metadata.get("name").cloned()
        .or_else(|| metadata.get("gameName").cloned())
        .unwrap_or(Value::String(appid.clone()));
    manifest["metadata"] = metadata.clone();
    manifest["installStatus"] = Value::String("installing".to_string());
    if let Ok(hash) = compute_object_hash(&metadata) {
        manifest["metadataHash"] = Value::String(hash);
    }
    manifest["installedAt"] = Value::Null;

    let _ = write_json_file(&manifest_path, &manifest);

    // Download remote image in background
    if let Some(image_url) = metadata.get("image").and_then(|v| v.as_str()) {
        if image_url.starts_with("http://") || image_url.starts_with("https://") {
            let image_url = image_url.to_string();
            let installing_root_clone = installing_root.clone();
            let manifest_path_clone = manifest_path.clone();
            let download_root_clone = download_root.clone();
            tauri::async_runtime::spawn(async move {
                if let Ok(resp) = reqwest::get(&image_url).await {
                    let ext = image_url.split('?').next().unwrap_or("")
                        .split('.').last().unwrap_or("png");
                    let image_name = format!("image.{}", &ext[..ext.len().min(8)]);
                    let image_path = installing_root_clone.join(&image_name);
                    if let Ok(bytes) = resp.bytes().await {
                        let _ = std::fs::write(&image_path, &bytes);
                        if let Some(mut m) = read_json_file(&manifest_path_clone) {
                            if let Some(meta) = m.get_mut("metadata") {
                                meta["localImage"] = Value::String(image_path.to_string_lossy().to_string());
                            }
                            let _ = write_json_file(&manifest_path_clone, &m);
                        }
                        update_installed_index(&download_root_clone.join(INSTALLED_DIR));
                    }
                }
            });
        }
    }

    serde_json::json!({ "ok": true })
}

#[tauri::command]
pub async fn installed_update_metadata(app: AppHandle, appid: String, updates: Value) -> Value {
    let roots = list_download_roots(&app);
    for base_root in roots {
        let root = base_root.join(INSTALLED_DIR);
        for (folder, _, _, _) in iterate_game_folders(&root) {
            let manifest_path = folder.join(INSTALLED_MANIFEST);
            let mut manifest = match read_json_file(&manifest_path) {
                Some(m) => m,
                None => continue,
            };
            if manifest.get("appid").and_then(|v| v.as_str()) != Some(&appid) {
                continue;
            }
            if manifest.get("metadata").is_none() {
                manifest["metadata"] = serde_json::json!({});
            }
            if let Some(meta) = manifest.get_mut("metadata") {
                if let Some(obj) = updates.as_object() {
                    for (k, v) in obj {
                        meta[k] = v.clone();
                    }
                }
            }
            if let Some(name) = updates.get("name") {
                manifest["name"] = name.clone();
            }
            if let Ok(hash) = compute_object_hash(&manifest["metadata"]) {
                manifest["metadataHash"] = Value::String(hash);
            }
            let _ = write_json_file(&manifest_path, &manifest);

            // Download new image if changed
            if let Some(image_url) = updates.get("image").and_then(|v| v.as_str()) {
                if image_url.starts_with("http://") || image_url.starts_with("https://") {
                    let image_url = image_url.to_string();
                    let folder_clone = folder.clone();
                    let manifest_path_clone = manifest_path.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Ok(resp) = reqwest::get(&image_url).await {
                            let ext = image_url.split('?').next().unwrap_or("")
                                .split('.').last().unwrap_or("png");
                            let image_name = format!("image.{}", &ext[..ext.len().min(8)]);
                            let image_path = folder_clone.join(&image_name);
                            if let Ok(bytes) = resp.bytes().await {
                                let _ = std::fs::write(&image_path, &bytes);
                                if let Some(mut m) = read_json_file(&manifest_path_clone) {
                                    if let Some(meta) = m.get_mut("metadata") {
                                        meta["localImage"] = Value::String(image_path.to_string_lossy().to_string());
                                    }
                                    let _ = write_json_file(&manifest_path_clone, &m);
                                }
                            }
                        }
                    });
                }
            }

            update_installed_index(&root);
            return serde_json::json!({ "ok": true });
        }
    }
    serde_json::json!({ "ok": false, "error": "Game not found in installed manifests" })
}

#[tauri::command]
pub fn installed_list(app: AppHandle) -> Vec<Value> {
    let root = ensure_download_dir(&app)
        .unwrap_or_else(|_| get_download_root(&app))
        .join(INSTALLED_DIR);
    list_manifests_from_root(&root, true)
}

#[tauri::command]
pub fn installed_get(app: AppHandle, appid: String) -> Option<Value> {
    let root = ensure_download_dir(&app)
        .unwrap_or_else(|_| get_download_root(&app))
        .join(INSTALLED_DIR);
    for (folder, _, _, _) in iterate_game_folders(&root) {
        let manifest_path = folder.join(INSTALLED_MANIFEST);
        if let Some(manifest) = read_json_file(&manifest_path) {
            if manifest.get("appid").and_then(|v| v.as_str()) == Some(&appid) {
                return Some(manifest);
            }
        }
    }
    None
}

#[tauri::command]
pub fn installed_list_by_appid(app: AppHandle, appid: String) -> Vec<Value> {
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    for root in list_download_roots(&app) {
        let installed_root = root.join(INSTALLED_DIR);
        for (folder, _, _, _) in iterate_game_folders(&installed_root) {
            let manifest_path = folder.join(INSTALLED_MANIFEST);
            if seen.contains(&manifest_path) { continue; }
            if let Some(mut manifest) = read_json_file(&manifest_path) {
                if manifest.get("appid").and_then(|v| v.as_str()) == Some(&appid) {
                    seen.insert(manifest_path);
                    manifest["installedFolder"] = Value::String(folder.to_string_lossy().to_string());
                    items.push(manifest);
                }
            }
        }
    }
    items
}

#[tauri::command]
pub fn installed_list_global(app: AppHandle) -> Vec<Value> {
    let mut best: HashMap<String, Value> = HashMap::new();
    for root in list_download_roots(&app) {
        let installed_root = root.join(INSTALLED_DIR);
        for item in list_manifests_from_root(&installed_root, true) {
            if let Some(appid) = item.get("appid").and_then(|v| v.as_str()) {
                let appid = appid.to_string();
                let existing = best.get(&appid);
                if existing.is_none() || manifest_richness(&item) > manifest_richness(existing.unwrap()) {
                    best.insert(appid, item);
                }
            }
        }
    }
    best.into_values().collect()
}

#[tauri::command]
pub fn installed_get_global(app: AppHandle, appid: String) -> Option<Value> {
    for root in list_download_roots(&app) {
        let installed_root = root.join(INSTALLED_DIR);
        for (folder, _, _, _) in iterate_game_folders(&installed_root) {
            let manifest_path = folder.join(INSTALLED_MANIFEST);
            if let Some(manifest) = read_json_file(&manifest_path) {
                if manifest.get("appid").and_then(|v| v.as_str()) == Some(&appid) {
                    return Some(manifest);
                }
            }
        }
    }
    None
}

#[tauri::command]
pub fn installing_list(app: AppHandle) -> Vec<Value> {
    let root = ensure_download_dir(&app)
        .unwrap_or_else(|_| get_download_root(&app))
        .join(INSTALLING_DIR);
    list_manifests_from_root(&root, false)
        .into_iter()
        .filter(|item| {
            let status = item.get("installStatus").and_then(|v| v.as_str()).unwrap_or("");
            !matches!(status, "completed" | "extracted" | "cancelled")
        })
        .collect()
}

#[tauri::command]
pub fn installing_get(app: AppHandle, appid: String) -> Option<Value> {
    let root = ensure_download_dir(&app)
        .unwrap_or_else(|_| get_download_root(&app))
        .join(INSTALLING_DIR);
    for (folder, _, _, _) in iterate_game_folders(&root) {
        let manifest_path = folder.join(INSTALLED_MANIFEST);
        if let Some(manifest) = read_json_file(&manifest_path) {
            if manifest.get("appid").and_then(|v| v.as_str()) == Some(&appid) {
                let status = manifest.get("installStatus").and_then(|v| v.as_str()).unwrap_or("");
                if matches!(status, "cancelled" | "completed" | "extracted") {
                    return None;
                }
                return Some(manifest);
            }
        }
    }
    None
}

#[tauri::command]
pub fn installing_list_global(app: AppHandle) -> Vec<Value> {
    let mut best: HashMap<String, Value> = HashMap::new();
    for root in list_download_roots(&app) {
        let installing_root = root.join(INSTALLING_DIR);
        for item in list_manifests_from_root(&installing_root, false) {
            let status = item.get("installStatus").and_then(|v| v.as_str()).unwrap_or("");
            if matches!(status, "completed" | "extracted" | "cancelled") { continue; }
            if let Some(appid) = item.get("appid").and_then(|v| v.as_str()) {
                let appid = appid.to_string();
                let existing = best.get(&appid);
                if existing.is_none() || manifest_richness(&item) > manifest_richness(existing.unwrap()) {
                    best.insert(appid, item);
                }
            }
        }
    }
    best.into_values().collect()
}

#[tauri::command]
pub fn installing_get_global(app: AppHandle, appid: String) -> Option<Value> {
    for root in list_download_roots(&app) {
        let installing_root = root.join(INSTALLING_DIR);
        for (folder, _, _, _) in iterate_game_folders(&installing_root) {
            let manifest_path = folder.join(INSTALLED_MANIFEST);
            if let Some(manifest) = read_json_file(&manifest_path) {
                if manifest.get("appid").and_then(|v| v.as_str()) == Some(&appid) {
                    let status = manifest.get("installStatus").and_then(|v| v.as_str()).unwrap_or("");
                    if matches!(status, "cancelled" | "completed" | "extracted") {
                        return None;
                    }
                    return Some(manifest);
                }
            }
        }
    }
    None
}

#[tauri::command]
pub fn installing_status_set(app: AppHandle, appid: String, status: String, error: Option<String>) -> Value {
    if let Some(folder) = find_installing_folder_by_appid(&app, &appid) {
        let manifest_path = folder.join(INSTALLED_MANIFEST);
        let mut manifest = read_json_file(&manifest_path).unwrap_or_else(|| serde_json::json!({
            "appid": appid,
            "name": appid
        }));
        manifest["installStatus"] = Value::String(status);
        if let Some(err) = error {
            manifest["installError"] = Value::String(err);
        }
        manifest["updatedAt"] = Value::Number(chrono::Utc::now().timestamp_millis().into());
        let ok = write_json_file(&manifest_path, &manifest).is_ok();
        return serde_json::json!({ "ok": ok });
    }
    serde_json::json!({ "ok": false })
}

#[tauri::command]
pub async fn installing_delete(app: AppHandle, appid: String) -> Value {
    for root in list_download_roots(&app) {
        let installing_root = root.join(INSTALLING_DIR);
        if !installing_root.exists() { continue; }
        for (folder, name, is_versioned, parent_folder) in iterate_game_folders(&installing_root) {
            let manifest_path = folder.join(INSTALLED_MANIFEST);
            let manifest = read_json_file(&manifest_path);
            let matches = manifest.as_ref()
                .and_then(|m| m.get("appid").and_then(|v| v.as_str()))
                .map(|id| id == appid)
                .unwrap_or(false)
                || name == appid;
            if matches {
                let to_delete = if is_versioned { parent_folder.unwrap_or(folder) } else { folder };
                let _ = tokio::fs::remove_dir_all(&to_delete).await;
                return serde_json::json!({ "ok": true });
            }
        }
    }
    serde_json::json!({ "ok": false })
}

#[tauri::command]
pub async fn installed_delete(app: AppHandle, appid: String) -> Value {
    for root in list_download_roots(&app) {
        let installed_root = root.join(INSTALLED_DIR);
        if !installed_root.exists() { continue; }
        for (folder, name, is_versioned, parent_folder) in iterate_game_folders(&installed_root) {
            let manifest_path = folder.join(INSTALLED_MANIFEST);
            let manifest = read_json_file(&manifest_path);
            let matches = manifest.as_ref()
                .and_then(|m| m.get("appid").and_then(|v| v.as_str()))
                .map(|id| id == appid)
                .unwrap_or(false)
                || name == appid;
            if matches {
                let to_delete = if is_versioned { parent_folder.unwrap_or(folder) } else { folder };
                let _ = tokio::fs::remove_dir_all(&to_delete).await;
                update_installed_index(&installed_root);
                return serde_json::json!({ "ok": true });
            }
        }
    }
    serde_json::json!({ "ok": false })
}

#[tauri::command]
pub async fn add_external_game(app: AppHandle, appid: String, metadata: Value, game_path: String) -> Value {
    let game_path = PathBuf::from(&game_path);
    if !game_path.exists() {
        return serde_json::json!({ "ok": false, "error": "The selected folder does not exist" });
    }

    let download_root = match ensure_download_dir(&app) {
        Ok(r) => r,
        Err(e) => return serde_json::json!({ "ok": false, "error": e.to_string() }),
    };

    let folder_name = safe_folder_name(
        metadata.get("name").and_then(|v| v.as_str())
            .or_else(|| metadata.get("gameName").and_then(|v| v.as_str()))
            .unwrap_or(&appid)
    );
    let installed_root = download_root.join(INSTALLED_DIR);
    std::fs::create_dir_all(&installed_root).unwrap_or_default();
    let game_folder = installed_root.join(&folder_name);
    std::fs::create_dir_all(&game_folder).unwrap_or_default();

    let manifest = serde_json::json!({
        "appid": appid,
        "name": metadata.get("name").cloned().unwrap_or(Value::String(appid.clone())),
        "metadata": metadata.clone(),
        "installStatus": "installed",
        "installedAt": chrono::Utc::now().timestamp_millis(),
        "addedAt": chrono::Utc::now().timestamp_millis(),
        "externalPath": game_path.to_string_lossy(),
        "isExternal": true,
        "metadataHash": compute_object_hash(&metadata).unwrap_or_default()
    });

    let manifest_path = game_folder.join(INSTALLED_MANIFEST);
    let _ = write_json_file(&manifest_path, &manifest);

    // Create symlink/junction to external game folder
    let link_path = game_folder.join("game");
    if link_path.exists() {
        let _ = std::fs::remove_dir_all(&link_path).or_else(|_| std::fs::remove_file(&link_path));
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::os::windows::fs::symlink_dir(&game_path, &link_path);
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = std::os::unix::fs::symlink(&game_path, &link_path);
    }

    // Download image in background
    if let Some(image_url) = metadata.get("image").and_then(|v| v.as_str()) {
        if image_url.starts_with("http://") || image_url.starts_with("https://") {
            let image_url = image_url.to_string();
            let game_folder_clone = game_folder.clone();
            let manifest_path_clone = manifest_path.clone();
            tauri::async_runtime::spawn(async move {
                if let Ok(resp) = reqwest::get(&image_url).await {
                    let ext = image_url.split('?').next().unwrap_or("")
                        .split('.').last().unwrap_or("png");
                    let image_name = format!("image.{}", &ext[..ext.len().min(8)]);
                    let image_path = game_folder_clone.join(&image_name);
                    if let Ok(bytes) = resp.bytes().await {
                        let _ = std::fs::write(&image_path, &bytes);
                        if let Some(mut m) = read_json_file(&manifest_path_clone) {
                            if let Some(meta) = m.get_mut("metadata") {
                                meta["localImage"] = Value::String(image_path.to_string_lossy().to_string());
                            }
                            let _ = write_json_file(&manifest_path_clone, &m);
                        }
                    }
                }
            });
        }
    }

    update_installed_index(&installed_root);
    serde_json::json!({ "ok": true })
}

#[tauri::command]
pub async fn pick_external_game_folder(app: AppHandle) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    let path = app.dialog()
        .file()
        .set_title("Select Game Folder")
        .blocking_pick_folder();
    path.map(|p| p.as_path().map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|| p.to_string()))
}

#[tauri::command]
pub async fn pick_image(app: AppHandle) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    let path = app.dialog()
        .file()
        .set_title("Select Image")
        .add_filter("Images", &["jpg", "jpeg", "png", "gif", "webp", "bmp"])
        .blocking_pick_file();
    path.map(|p| p.as_path().map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|| p.to_string()))
}

#[tauri::command]
pub async fn network_test(_app: AppHandle, base_url: Option<String>) -> Value {
    let origin = base_url.as_deref().unwrap_or("https://union-crax.xyz");
    let targets = vec![
        ("API base", origin.to_string()),
        ("API downloads", format!("{}/api/downloads/all", origin)),
        ("Pixeldrain", "https://pixeldrain.com".to_string()),
        ("FileQ", "https://fileq.net".to_string()),
        ("DataVaults", "https://datavaults.co".to_string()),
        ("Rootz", "https://rootz.so".to_string()),
    ];

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(6))
        .build()
        .unwrap_or_default();

    let mut results = Vec::new();
    for (label, url) in targets {
        let start = Instant::now();
        let result = client.get(&url).send().await;
        let elapsed_ms = start.elapsed().as_millis() as u64;
        match result {
            Ok(resp) => results.push(serde_json::json!({
                "label": label,
                "url": url,
                "ok": resp.status().is_success(),
                "status": resp.status().as_u16(),
                "elapsedMs": elapsed_ms
            })),
            Err(e) => results.push(serde_json::json!({
                "label": label,
                "url": url,
                "ok": false,
                "status": 0,
                "error": e.to_string(),
                "elapsedMs": elapsed_ms
            })),
        }
    }

    serde_json::json!({ "ok": true, "results": results })
}
