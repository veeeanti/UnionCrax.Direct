/// Discord Rich Presence module using the discord-rpc-client crate.
/// Runs the RPC client in a background thread to avoid blocking the async runtime.
use serde_json::Value;
use std::sync::{Arc, Mutex};
use once_cell::sync::Lazy;
use tauri::AppHandle;

const RPC_CLIENT_ID: &str = "1464971744199839928";

#[derive(Debug, Clone, Default)]
struct RpcActivity {
    details: Option<String>,
    state: Option<String>,
    start_timestamp: Option<i64>,
    end_timestamp: Option<i64>,
    large_image_key: Option<String>,
    large_image_text: Option<String>,
    small_image_key: Option<String>,
    small_image_text: Option<String>,
    buttons: Option<Vec<(String, String)>>,
}

#[derive(Debug)]
struct RpcState {
    enabled: bool,
    ready: bool,
    last_activity: Option<RpcActivity>,
    game_activity: Option<RpcActivity>,
    window_hidden: bool,
    sender: Option<std::sync::mpsc::Sender<RpcCommand>>,
}

#[derive(Debug)]
enum RpcCommand {
    SetActivity(RpcActivity),
    ClearActivity,
    Shutdown,
}

static RPC_STATE: Lazy<Mutex<RpcState>> = Lazy::new(|| {
    Mutex::new(RpcState {
        enabled: true,
        ready: false,
        last_activity: None,
        game_activity: None,
        window_hidden: false,
        sender: None,
    })
});

fn parse_activity(payload: &Value) -> RpcActivity {
    let buttons = payload.get("buttons").and_then(|b| b.as_array()).map(|arr| {
        arr.iter().filter_map(|btn| {
            let label = btn.get("label").and_then(|v| v.as_str())?.to_string();
            let url = btn.get("url").and_then(|v| v.as_str())?.to_string();
            Some((label, url))
        }).collect::<Vec<_>>()
    });

    RpcActivity {
        details: payload.get("details").and_then(|v| v.as_str()).map(|s| s.to_string()),
        state: payload.get("state").and_then(|v| v.as_str()).map(|s| s.to_string()),
        start_timestamp: payload.get("startTimestamp").and_then(|v| v.as_i64()),
        end_timestamp: payload.get("endTimestamp").and_then(|v| v.as_i64()),
        large_image_key: payload.get("largeImageKey").and_then(|v| v.as_str()).map(|s| s.to_string()),
        large_image_text: payload.get("largeImageText").and_then(|v| v.as_str()).map(|s| s.to_string()),
        small_image_key: payload.get("smallImageKey").and_then(|v| v.as_str()).map(|s| s.to_string()),
        small_image_text: payload.get("smallImageText").and_then(|v| v.as_str()).map(|s| s.to_string()),
        buttons,
    }
}

fn start_rpc_thread() -> std::sync::mpsc::Sender<RpcCommand> {
    let (tx, rx) = std::sync::mpsc::channel::<RpcCommand>();

    std::thread::spawn(move || {
        use discord_rpc_client::Client;

        let mut client = Client::new(RPC_CLIENT_ID.parse().unwrap_or(0));
        client.on_ready(|_ctx| {
            let mut state = RPC_STATE.lock().unwrap();
            state.ready = true;
            eprintln!("[UC] Discord RPC connected");
        });
        client.on_error(|err| {
            eprintln!("[UC] Discord RPC error: {:?}", err);
        });

        // start() is non-blocking and returns ()
        client.start();

        loop {
            match rx.recv() {
                Ok(RpcCommand::SetActivity(activity)) => {
                    let result = client.set_activity(|a| {
                        let mut act = a;
                        if let Some(ref details) = activity.details {
                            act = act.details(details);
                        }
                        if let Some(ref state_str) = activity.state {
                            act = act.state(state_str);
                        }
                        if let Some(ts) = activity.start_timestamp {
                            act = act.timestamps(|t| t.start(ts as u64));
                        }
                        if let Some(ref key) = activity.large_image_key {
                            let text = activity.large_image_text.as_deref().unwrap_or("");
                            act = act.assets(|a| a.large_image(key).large_text(text));
                        }
                        act
                    });
                    if let Err(e) = result {
                        eprintln!("[UC] Discord RPC set_activity failed: {:?}", e);
                    }
                }
                Ok(RpcCommand::ClearActivity) => {
                    if let Err(e) = client.clear_activity() {
                        eprintln!("[UC] Discord RPC clear_activity failed: {:?}", e);
                    }
                }
                Ok(RpcCommand::Shutdown) | Err(_) => {
                    let _ = client.clear_activity();
                    let mut state = RPC_STATE.lock().unwrap();
                    state.ready = false;
                    break;
                }
            }
        }
    });

    tx
}

fn ensure_rpc_client() {
    let mut state = RPC_STATE.lock().unwrap();
    if !state.enabled {
        return;
    }
    if state.sender.is_none() {
        let tx = start_rpc_thread();
        state.sender = Some(tx);
    }
}

fn send_rpc_command(cmd: RpcCommand) {
    let state = RPC_STATE.lock().unwrap();
    if let Some(ref tx) = state.sender {
        let _ = tx.send(cmd);
    }
}

pub fn shutdown_rpc() {
    let mut state = RPC_STATE.lock().unwrap();
    if let Some(tx) = state.sender.take() {
        let _ = tx.send(RpcCommand::Shutdown);
    }
    state.ready = false;
}

pub fn set_game_rpc_activity(payload: &Value) {
    let activity = parse_activity(payload);
    {
        let mut state = RPC_STATE.lock().unwrap();
        state.game_activity = Some(activity.clone());
        if state.window_hidden || !state.enabled { return; }
    }
    ensure_rpc_client();
    send_rpc_command(RpcCommand::SetActivity(activity));
}

pub fn clear_game_rpc_activity() {
    let last = {
        let mut state = RPC_STATE.lock().unwrap();
        state.game_activity = None;
        state.last_activity.clone()
    };
    if let Some(activity) = last {
        ensure_rpc_client();
        send_rpc_command(RpcCommand::SetActivity(activity));
    } else {
        send_rpc_command(RpcCommand::ClearActivity);
    }
}

pub fn hide_rpc_activity() {
    let mut state = RPC_STATE.lock().unwrap();
    state.window_hidden = true;
    if let Some(ref tx) = state.sender {
        let _ = tx.send(RpcCommand::ClearActivity);
    }
}

pub fn restore_rpc_activity() {
    let activity = {
        let mut state = RPC_STATE.lock().unwrap();
        if !state.window_hidden { return; }
        state.window_hidden = false;
        state.game_activity.clone().or_else(|| state.last_activity.clone())
    };
    if let Some(act) = activity {
        ensure_rpc_client();
        send_rpc_command(RpcCommand::SetActivity(act));
    }
}

// ── Tauri commands ────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn rpc_set_activity(_app: AppHandle, payload: Value) -> Value {
    let activity = parse_activity(&payload);
    {
        let mut state = RPC_STATE.lock().unwrap();
        if !state.enabled { return serde_json::json!({ "ok": true }); }
        state.last_activity = Some(activity.clone());
        if state.window_hidden { return serde_json::json!({ "ok": true }); }
        // If there's a game activity, don't override it
        if state.game_activity.is_some() { return serde_json::json!({ "ok": true }); }
    }
    ensure_rpc_client();
    send_rpc_command(RpcCommand::SetActivity(activity));
    serde_json::json!({ "ok": true })
}

#[tauri::command]
pub async fn rpc_clear(_app: AppHandle) -> Value {
    {
        let mut state = RPC_STATE.lock().unwrap();
        state.last_activity = None;
        // If there's a game activity, don't clear
        if state.game_activity.is_some() { return serde_json::json!({ "ok": true }); }
    }
    send_rpc_command(RpcCommand::ClearActivity);
    serde_json::json!({ "ok": true })
}

#[tauri::command]
pub async fn rpc_status(_app: AppHandle) -> Value {
    let state = RPC_STATE.lock().unwrap();
    serde_json::json!({
        "ok": true,
        "enabled": state.enabled,
        "ready": state.ready,
        "clientId": RPC_CLIENT_ID
    })
}

/// Update RPC settings (called when discordRpcEnabled setting changes)
pub fn update_rpc_settings(enabled: bool) {
    let mut state = RPC_STATE.lock().unwrap();
    state.enabled = enabled;
    if !enabled {
        if let Some(ref tx) = state.sender {
            let _ = tx.send(RpcCommand::Shutdown);
        }
        state.sender = None;
        state.ready = false;
    }
}
