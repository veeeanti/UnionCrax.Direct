# UnionCrax.Direct — Tauri Edition

This is the **Tauri v2** rewrite of the UnionCrax.Direct desktop app.
The original Electron version lives in the parent directory (`../electron/`).

## Architecture

```
tauri-app/
├── package.json                    # Frontend deps + @tauri-apps/* packages
├── .npmrc                          # Use npm (not pnpm) for this project
├── .gitignore
├── README.md                       # This file
├── renderer/                       # React frontend (shared with Electron)
│   ├── index.html
│   ├── vite.config.ts              # Tauri-aware Vite config
│   ├── tsconfig.json
│   ├── postcss.config.cjs
│   ├── tailwind.config.ts
│   ├── public/                     # Fonts, icons, banner
│   └── src/
│       ├── main.tsx                # Calls installTauriBridge() before render
│       ├── lib/tauri-bridge.ts     # KEY FILE: replaces Electron preload
│       └── ...                     # All other files identical to Electron
└── src-tauri/                      # Rust backend (replaces electron/main.cjs)
    ├── Cargo.toml
    ├── build.rs
    ├── tauri.conf.json
    ├── capabilities/default.json   # Tauri v2 plugin permissions
    └── src/
        ├── main.rs                 # Entry point
        ├── lib.rs                  # App setup + command registration
        ├── settings.rs             # Settings read/write
        ├── logging.rs              # File-based logging
        ├── downloads.rs            # Download management, extraction, manifests
        ├── games.rs                # Exe discovery, launching, shortcuts
        ├── auth.rs                 # Discord OAuth via embedded WebView
        ├── rpc.rs                  # Discord RPC stub
        ├── updater.rs              # GitHub release checks
        └── tray.rs                 # System tray
```

## Key differences from Electron

| Feature | Electron | Tauri |
|---------|----------|-------|
| Backend language | Node.js (CJS) | Rust |
| IPC | `contextBridge` + `ipcMain.handle` | `#[tauri::command]` + `invoke()` |
| Downloads | Chromium `will-download` event | `reqwest` streaming |
| Extraction | `7zip-bin` npm package | System `7z`/`7za` binary |
| Auth window | `BrowserWindow` | `WebviewWindowBuilder` |
| Settings | JSON file via `fs` | JSON file via `std::fs` |
| Tray | `Tray` + `Menu` | `TrayIconBuilder` |
| Events | `win.webContents.send()` | `app.emit()` |
| Discord RPC | `discord-rpc` npm package | Stub (extend with `discord-rpc` crate) |

## Prerequisites

- [Rust](https://rustup.rs/) (stable)
- [Node.js](https://nodejs.org/) 18+
- On Windows: [WebView2](https://developer.microsoft.com/en-us/microsoft-edge/webview2/) (usually pre-installed on Windows 10/11)
- On Linux: `webkit2gtk-4.1`, `libssl-dev`, `libgtk-3-dev`, `libayatana-appindicator3-dev`
- 7-zip (`7z` or `7za`) on system PATH for archive extraction

## Development

> **Important:** Use `npm` (not `pnpm`) inside this directory. The root workspace's `pnpm-workspace.yaml` interferes with `@tauri-apps/cli`'s platform-specific binary installation.

```bash
cd tauri-app
npm install
npm run dev
```

This runs `tauri dev` which:
1. Starts the Vite dev server on `http://localhost:5173`
2. Compiles the Rust backend
3. Opens the app window pointing at the dev server

## Building

```bash
cd tauri-app
npm install
npm run build
```

Produces installers in `src-tauri/target/release/bundle/`.

## Notes

- The renderer source files are **shared** with the Electron version — only `main.tsx` differs (it calls `installTauriBridge()` first).
- The `tauri-bridge.ts` file maps all `window.ucDownloads`, `window.ucSettings`, etc. calls to Tauri `invoke()` commands, so the rest of the renderer code works without modification.
- Discord RPC (`rpc.rs`) is a stub — the activity is stored in memory but not sent to Discord. To enable it, integrate the [`discord-rpc`](https://crates.io/crates/discord-rpc-client) crate.
- The `auth.rs` module opens a WebView window for Discord OAuth. Session cookies are managed by the WebView's cookie store.
