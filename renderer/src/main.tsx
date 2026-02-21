import React from 'react'
import ReactDOM from 'react-dom/client'
import './fonts.css'
import App from './app/App'
import './globals.css'
import { installTauriBridge } from './lib/tauri-bridge'

// Install the Tauri bridge before rendering so all window.uc* globals are available
installTauriBridge()

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
)
