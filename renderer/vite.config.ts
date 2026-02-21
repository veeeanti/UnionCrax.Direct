import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import { resolve } from 'node:path'

// https://vitejs.dev/config/
export default defineConfig({
  root: __dirname,
  base: './',
  plugins: [react()],
  resolve: {
    alias: {
      '@': resolve(__dirname, 'src')
    }
  },
  // Tauri dev server
  server: {
    port: 5173,
    strictPort: true,
    host: 'localhost',
    fs: { strict: true }
  },
  // Tauri expects a static build
  build: {
    outDir: resolve(__dirname, 'dist'),
    emptyOutDir: true,
    // Tauri uses Chromium on Windows/Linux, WebKit on macOS
    target: process.env.TAURI_ENV_PLATFORM === 'windows'
      ? 'chrome105'
      : process.env.TAURI_ENV_PLATFORM === 'macos'
        ? 'safari13'
        : 'chrome105',
    minify: process.env.TAURI_ENV_DEBUG ? false : 'esbuild',
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
  // Prevent vite from obscuring Rust errors
  clearScreen: false,
  envPrefix: ['VITE_', 'TAURI_ENV_'],
})
