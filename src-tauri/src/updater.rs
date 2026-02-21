/// Updater module — checks GitHub releases and opens the releases page.
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};

const RELEASES_URL: &str = "https://github.com/Union-Crax/UnionCrax.Direct/releases/latest";
const GITHUB_API_URL: &str = "https://api.github.com/repos/Union-Crax/UnionCrax.Direct/releases/latest";

fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let parse = |s: &str| -> Vec<u64> {
        s.trim_start_matches('v')
            .split('.')
            .map(|n| n.parse::<u64>().unwrap_or(0))
            .collect()
    };
    let pa = parse(a);
    let pb = parse(b);
    let len = pa.len().max(pb.len());
    for i in 0..len {
        let da = pa.get(i).copied().unwrap_or(0);
        let db = pb.get(i).copied().unwrap_or(0);
        match da.cmp(&db) {
            std::cmp::Ordering::Equal => continue,
            other => return other,
        }
    }
    std::cmp::Ordering::Equal
}

async fn fetch_latest_release() -> anyhow::Result<(String, String)> {
    let client = reqwest::Client::builder()
        .user_agent("UnionCrax.Direct")
        .build()?;
    let resp = client.get(GITHUB_API_URL)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("HTTP {}", resp.status());
    }
    let data: Value = resp.json().await?;
    let tag = data.get("tag_name")
        .or_else(|| data.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let url = data.get("html_url")
        .and_then(|v| v.as_str())
        .unwrap_or(RELEASES_URL)
        .to_string();
    let latest = tag.trim_start_matches('v').to_string();
    Ok((latest, url))
}

pub async fn check_for_updates_silent(app: &AppHandle) {
    let current = app.package_info().version.to_string();
    match fetch_latest_release().await {
        Ok((latest, url)) => {
            if compare_versions(&latest, &current) == std::cmp::Ordering::Greater {
                let info = serde_json::json!({ "version": latest, "url": url });
                let _ = app.emit("update-available", info);
            } else {
                let _ = app.emit("update-not-available", serde_json::json!({ "version": current }));
            }
        }
        Err(e) => {
            eprintln!("[UC] Release check error: {}", e);
        }
    }
}

#[tauri::command]
pub async fn check_for_updates(app: AppHandle) -> Value {
    use tauri_plugin_opener::OpenerExt;
    let current = app.package_info().version.to_string();
    match fetch_latest_release().await {
        Ok((latest, url)) => {
            if compare_versions(&latest, &current) == std::cmp::Ordering::Greater {
                let _ = app.opener().open_url(&url, None::<&str>);
                serde_json::json!({ "ok": true, "url": url, "latest": latest, "current": current })
            } else {
                serde_json::json!({ "ok": false, "upToDate": true, "current": current })
            }
        }
        Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
    }
}

#[tauri::command]
pub async fn update_retry(app: AppHandle) -> Value {
    check_for_updates(app).await
}

#[tauri::command]
pub fn get_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}
