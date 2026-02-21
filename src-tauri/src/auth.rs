/// Auth module — Discord OAuth via an embedded WebView window.
/// Maintains a shared cookie store for authenticated API requests.
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use once_cell::sync::Lazy;
use tauri::{AppHandle, Listener, WebviewWindowBuilder, WebviewUrl};

const DEFAULT_BASE_URL: &str = "https://union-crax.xyz";

// Shared cookie store: maps domain -> cookie string
static COOKIE_STORE: Lazy<Mutex<HashMap<String, String>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

fn normalize_base_url(base_url: Option<&str>) -> String {
    match base_url {
        Some(url) if !url.is_empty() => {
            if let Ok(parsed) = url::Url::parse(url) {
                return parsed.origin().ascii_serialization();
            }
            DEFAULT_BASE_URL.to_string()
        }
        _ => DEFAULT_BASE_URL.to_string(),
    }
}

fn build_auth_url(base_url: &str, next_path: &str) -> String {
    format!("{}/api/discord/connect?next={}", base_url, urlencoding::encode(next_path))
}

fn parse_auth_result(url_str: &str) -> Option<bool> {
    if let Ok(parsed) = url::Url::parse(url_str) {
        let params: HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        if let Some(connected) = params.get("discord_connected") {
            if connected == "true" || connected == "1" {
                return Some(true);
            }
        }
        if params.contains_key("error") {
            return Some(false);
        }
    }
    None
}

/// Store cookies for a domain (called after successful auth)
pub fn store_cookies(domain: &str, cookies: &str) {
    let mut store = COOKIE_STORE.lock().unwrap();
    store.insert(domain.to_string(), cookies.to_string());
}

/// Get stored cookies for a domain
pub fn get_stored_cookies(domain: &str) -> Option<String> {
    let store = COOKIE_STORE.lock().unwrap();
    store.get(domain).cloned()
}

#[tauri::command]
pub async fn auth_login(app: AppHandle, base_url: Option<String>) -> Value {
    let origin = normalize_base_url(base_url.as_deref());
    let auth_url = build_auth_url(&origin, "/settings");

    let result: Arc<Mutex<Option<bool>>> = Arc::new(Mutex::new(None));
    let result_clone = result.clone();

    let auth_window = WebviewWindowBuilder::new(
        &app,
        "auth",
        WebviewUrl::External(auth_url.parse().unwrap()),
    )
    .title("Discord Login")
    .inner_size(520.0, 720.0)
    .resizable(false)
    .build();

    match auth_window {
        Ok(win) => {
            let result_close = result_clone.clone();
            win.on_window_event(move |event| {
                if let tauri::WindowEvent::CloseRequested { .. } = event {
                    let mut r = result_close.lock().unwrap();
                    if r.is_none() {
                        *r = Some(false);
                    }
                }
            });

            // Listen for the auth result event
            let result_event = result_clone.clone();
            let _unlisten = app.listen("uc:auth-result", move |event| {
                if let Ok(payload) = serde_json::from_str::<Value>(event.payload()) {
                    let ok = payload.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
                    let mut r = result_event.lock().unwrap();
                    if r.is_none() {
                        *r = Some(ok);
                    }
                }
            });

            // Poll for result with timeout
            let timeout = std::time::Duration::from_secs(300);
            let start = std::time::Instant::now();
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                let r = result.lock().unwrap().clone();
                if let Some(ok) = r {
                    let _ = win.close();
                    if ok {
                        return serde_json::json!({ "ok": true });
                    } else {
                        return serde_json::json!({ "ok": false, "error": "auth_failed" });
                    }
                }
                if start.elapsed() > timeout {
                    let _ = win.close();
                    return serde_json::json!({ "ok": false, "error": "timeout" });
                }
            }
        }
        Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
    }
}

#[tauri::command]
pub async fn auth_logout(_app: AppHandle, base_url: Option<String>) -> Value {
    let origin = normalize_base_url(base_url.as_deref());
    // Clear stored cookies
    {
        let mut store = COOKIE_STORE.lock().unwrap();
        store.remove(&origin);
    }
    let logout_url = format!("{}/api/discord/logout", origin);
    match reqwest::Client::new().post(&logout_url).send().await {
        Ok(_) => serde_json::json!({ "ok": true }),
        Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
    }
}

#[tauri::command]
pub async fn auth_session(_app: AppHandle, base_url: Option<String>) -> Value {
    let origin = normalize_base_url(base_url.as_deref());
    let session_url = format!("{}/api/discord/session", origin);

    let mut req = reqwest::Client::new().get(&session_url);
    if let Some(cookies) = get_stored_cookies(&origin) {
        req = req.header("Cookie", cookies);
    }

    match req.send().await {
        Ok(resp) if resp.status().is_success() => {
            match resp.json::<Value>().await {
                Ok(data) => data,
                Err(_) => serde_json::json!({ "discordId": null }),
            }
        }
        _ => serde_json::json!({ "discordId": null }),
    }
}

/// Store cookies from the renderer (called when renderer has cookies to share)
#[tauri::command]
pub fn auth_store_cookies(domain: String, cookies: String) -> Value {
    store_cookies(&domain, &cookies);
    serde_json::json!({ "ok": true })
}

#[tauri::command]
pub async fn auth_fetch(
    _app: AppHandle,
    base_url: String,
    path: String,
    init: Option<Value>,
) -> Value {
    let origin = normalize_base_url(Some(&base_url));
    let url = format!("{}{}", origin, path);

    let method = init.as_ref()
        .and_then(|i| i.get("method"))
        .and_then(|m| m.as_str())
        .unwrap_or("GET")
        .to_uppercase();

    let client = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap_or_default();

    let mut req = match method.as_str() {
        "POST" => client.post(&url),
        "PUT" => client.put(&url),
        "DELETE" => client.delete(&url),
        "PATCH" => client.patch(&url),
        _ => client.get(&url),
    };

    // Forward stored cookies
    if let Some(cookies) = get_stored_cookies(&origin) {
        if !cookies.is_empty() {
            req = req.header("Cookie", cookies);
        }
    }

    req = req.header("User-Agent", format!("UnionCrax.Direct/{}", env!("CARGO_PKG_VERSION")));
    req = req.header("Origin", &origin);
    req = req.header("Referer", format!("{}/", origin));

    if let Some(headers) = init.as_ref().and_then(|i| i.get("headers")).and_then(|h| h.as_object()) {
        for (k, v) in headers {
            if let Some(v_str) = v.as_str() {
                req = req.header(k, v_str);
            }
        }
    }

    if let Some(body) = init.as_ref().and_then(|i| i.get("body")).and_then(|b| b.as_str()) {
        req = req.body(body.to_string());
    }

    match req.send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let ok = resp.status().is_success();
            let status_text = resp.status().canonical_reason().unwrap_or("").to_string();
            // Extract Set-Cookie headers to update our cookie store
            let set_cookies: Vec<String> = resp.headers()
                .get_all("set-cookie")
                .iter()
                .filter_map(|v| v.to_str().ok())
                .map(|s| s.split(';').next().unwrap_or("").to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !set_cookies.is_empty() {
                let mut store = COOKIE_STORE.lock().unwrap();
                let existing = store.entry(origin.clone()).or_default();
                // Merge new cookies
                let mut cookie_map: HashMap<String, String> = existing
                    .split("; ")
                    .filter(|s| !s.is_empty())
                    .filter_map(|s| {
                        let mut parts = s.splitn(2, '=');
                        Some((parts.next()?.to_string(), parts.next().unwrap_or("").to_string()))
                    })
                    .collect();
                for cookie in &set_cookies {
                    let mut parts = cookie.splitn(2, '=');
                    if let (Some(k), Some(v)) = (parts.next(), parts.next()) {
                        cookie_map.insert(k.to_string(), v.to_string());
                    }
                }
                *existing = cookie_map.iter()
                    .map(|(k, v)| format!("{}={}", k, v))
                    .collect::<Vec<_>>()
                    .join("; ");
            }
            let headers: Vec<(String, String)> = resp.headers()
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
                .collect();
            match resp.bytes().await {
                Ok(bytes) => {
                    use base64::Engine;
                    let body = base64::engine::general_purpose::STANDARD.encode(&bytes);
                    serde_json::json!({
                        "ok": ok,
                        "status": status,
                        "statusText": status_text,
                        "headers": headers,
                        "body": body
                    })
                }
                Err(_) => serde_json::json!({ "ok": false, "status": 0, "statusText": "read_failed", "headers": [], "body": "" }),
            }
        }
        Err(_) => serde_json::json!({ "ok": false, "status": 0, "statusText": "fetch_failed", "headers": [], "body": "" }),
    }
}
