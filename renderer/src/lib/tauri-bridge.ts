/**
 * tauri-bridge.ts
 *
 * Replaces the Electron preload/contextBridge pattern.
 * All window.ucDownloads / window.ucSettings / etc. calls are re-routed
 * through Tauri's invoke() and event system.
 *
 * This file is imported once in main.tsx and populates window.* globals
 * so the rest of the renderer code works without modification.
 */

import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'

// ── ucDownloads ───────────────────────────────────────────────────────────────

const ucDownloads = {
  start: (payload: any) => invoke('download_start', { payload }),
  cancel: (downloadId: string) => invoke('download_cancel', { downloadId }),
  pause: (downloadId: string) => invoke('download_pause', { downloadId }),
  resume: (downloadId: string) => invoke('download_resume', { downloadId }),
  resumeInterrupted: (payload: any) => invoke('download_resume_interrupted', { payload }),
  resumeWithFreshUrl: (payload: any) => invoke('download_resume_with_fresh_url', { payload }),
  showInFolder: (targetPath: string) => invoke('download_show', { targetPath }),
  openPath: (targetPath: string) => invoke('download_open', { targetPath }),
  listDisks: () => invoke('disk_list'),
  getDownloadPath: () => invoke('download_path_get'),
  setDownloadPath: (targetPath: string) => invoke('download_path_set', { targetPath }),
  pickDownloadPath: () => invoke('download_path_pick'),
  getDownloadUsage: (targetPath?: string) => invoke('download_usage', { targetPath }),
  clearDownloadCache: () => invoke('download_cache_clear'),
  // Installed manifests
  saveInstalledMetadata: (appid: string, metadata: any) => invoke('installed_save', { appid, metadata }),
  listInstalled: () => invoke('installed_list'),
  getInstalled: (appid: string) => invoke('installed_get', { appid }),
  listInstalledByAppid: (appid: string) => invoke('installed_list_by_appid', { appid }),
  listInstalling: () => invoke('installing_list'),
  getInstalling: (appid: string) => invoke('installing_get', { appid }),
  listInstalledGlobal: () => invoke('installed_list_global'),
  getInstalledGlobal: (appid: string) => invoke('installed_get_global', { appid }),
  listInstallingGlobal: () => invoke('installing_list_global'),
  getInstallingGlobal: (appid: string) => invoke('installing_get_global', { appid }),
  listGameExecutables: (appid: string, versionLabel?: string | null) =>
    invoke('game_exe_list', { appid, versionLabel }),
  findGameSubfolder: (folder: string) => invoke('game_subfolder_find', { folder }),
  browseForGameExe: (defaultPath?: string) => invoke('game_browse_exe', { defaultPath }),
  launchGameExecutable: (appid: string, exePath: string, gameName?: string, showGameName?: boolean) =>
    invoke('game_exe_launch', { appid, exePath, gameName, showGameName }),
  launchGameExecutableAsAdmin: (appid: string, exePath: string, gameName?: string, showGameName?: boolean) =>
    invoke('game_exe_launch_admin', { appid, exePath, gameName, showGameName }),
  getRunningGame: (appid: string) => invoke('game_exe_running', { appid }),
  quitGameExecutable: (appid: string) => invoke('game_exe_quit', { appid }),
  deleteInstalled: (appid: string) => invoke('installed_delete', { appid }),
  deleteInstalling: (appid: string) => invoke('installing_delete', { appid }),
  setInstallingStatus: (appid: string, status: string, error?: string | null) =>
    invoke('installing_status_set', { appid, status, error }),
  createDesktopShortcut: (gameName: string, exePath: string) =>
    invoke('create_desktop_shortcut', { gameName, exePath }),
  deleteDesktopShortcut: (gameName: string) => invoke('delete_desktop_shortcut', { gameName }),
  addExternalGame: (appid: string, metadata: any, gamePath: string) =>
    invoke('add_external_game', { appid, metadata, gamePath }),
  updateInstalledMetadata: (appid: string, updates: Record<string, any>) =>
    invoke('installed_update_metadata', { appid, updates }),
  pickExternalGameFolder: () => invoke('pick_external_game_folder'),
  pickImage: () => invoke('pick_image'),
  onUpdate: (callback: (update: any) => void): (() => void) => {
    let unlisten: UnlistenFn | null = null
    listen('uc:download-update', (event) => {
      callback(event.payload)
    }).then((fn) => {
      unlisten = fn
    })
    return () => {
      if (unlisten) unlisten()
    }
  },
}

// ── ucSettings ────────────────────────────────────────────────────────────────

const ucSettings = {
  get: (key: string) => invoke('setting_get', { key }),
  set: (key: string, value: any) => invoke('setting_set', { key, value }),
  clearAll: () => invoke('setting_clear_all'),
  exportSettings: () => invoke('settings_export'),
  importSettings: () => invoke('settings_import'),
  runNetworkTest: (baseUrl?: string) => invoke('network_test', { baseUrl }),
  onChanged: (callback: (data: { key: string; value: any }) => void): (() => void) => {
    let unlisten: UnlistenFn | null = null
    listen('uc:setting-changed', (event) => {
      callback(event.payload as { key: string; value: any })
    }).then((fn) => {
      unlisten = fn
    })
    return () => {
      if (unlisten) unlisten()
    }
  },
}

// ── ucAuth ────────────────────────────────────────────────────────────────────

const ucAuth = {
  login: (baseUrl?: string) => invoke('auth_login', { baseUrl }),
  logout: (baseUrl?: string) => invoke('auth_logout', { baseUrl }),
  getSession: (baseUrl?: string) => invoke('auth_session', { baseUrl }),
  fetch: (baseUrl: string, path: string, init?: any) =>
    invoke('auth_fetch', { baseUrl, path, init }),
  storeCookies: (domain: string, cookies: string) =>
    invoke('auth_store_cookies', { domain, cookies }),
}

// ── ucUpdater ─────────────────────────────────────────────────────────────────

const ucUpdater = {
  checkForUpdates: () => invoke('check_for_updates'),
  installUpdate: () => {},
  getVersion: () => invoke('get_version'),
  getUpdateStatus: () => Promise.resolve(null),
  retryUpdate: () => invoke('update_retry'),
}

// ── ucLogs ────────────────────────────────────────────────────────────────────

const ucLogs = {
  log: (level: string, message: string, data?: any) =>
    invoke('log_message', { level, message, data }),
  getLogs: () => invoke('logs_get'),
  clearLogs: () => invoke('logs_clear'),
  openLogsFolder: () => invoke('logs_open_folder'),
}

// ── ucRpc ─────────────────────────────────────────────────────────────────────

const ucRpc = {
  setActivity: (payload: any) => invoke('rpc_set_activity', { payload }),
  clearActivity: () => invoke('rpc_clear'),
  getStatus: () => invoke('rpc_status'),
}

// ── electron compat shim ──────────────────────────────────────────────────────
// Some renderer code uses window.electron.ipcRenderer.on/removeListener for
// update-available / update-not-available events. We bridge those to Tauri events.

const electronShim = {
  ipcRenderer: {
    _listeners: new Map<string, Set<(...args: any[]) => void>>(),
    _unlisteners: new Map<string, UnlistenFn>(),
    on(channel: string, func: (...args: any[]) => void) {
      if (!this._listeners.has(channel)) {
        this._listeners.set(channel, new Set())
        // Subscribe to Tauri event
        listen(channel, (event) => {
          const listeners = electronShim.ipcRenderer._listeners.get(channel)
          if (listeners) {
            for (const fn of listeners) {
              fn(null, event.payload)
            }
          }
        }).then((unlisten) => {
          this._unlisteners.set(channel, unlisten)
        })
      }
      this._listeners.get(channel)!.add(func)
    },
    removeListener(channel: string, func: (...args: any[]) => void) {
      const listeners = this._listeners.get(channel)
      if (listeners) {
        listeners.delete(func)
        if (listeners.size === 0) {
          this._listeners.delete(channel)
          const unlisten = this._unlisteners.get(channel)
          if (unlisten) {
            unlisten()
            this._unlisteners.delete(channel)
          }
        }
      }
    },
  },
}

// ── Install on window ─────────────────────────────────────────────────────────

declare global {
  interface Window {
    ucDownloads?: typeof ucDownloads
    ucSettings?: typeof ucSettings
    ucAuth?: typeof ucAuth
    ucUpdater?: typeof ucUpdater
    ucLogs?: typeof ucLogs
    ucRpc?: typeof ucRpc
    electron?: typeof electronShim
  }
}

export function installTauriBridge() {
  window.ucDownloads = ucDownloads as any
  window.ucSettings = ucSettings as any
  window.ucAuth = ucAuth as any
  window.ucUpdater = ucUpdater as any
  window.ucLogs = ucLogs as any
  window.ucRpc = ucRpc as any
  window.electron = electronShim as any
}
