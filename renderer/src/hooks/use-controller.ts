import { useState, useEffect, useCallback, useRef } from 'react'

// Controller types
export interface ControllerInfo {
  index: number
  id: string
  type: string
  model: string
  name: string
  hasCustomBinds: boolean
}

export interface ControllerButtonState {
  pressed: boolean
  value: number
  touched: boolean
}

export interface ControllerState {
  connected: boolean
  index: number
  id: string
  timestamp: number
  buttons: Record<string, ControllerButtonState>
  axes: Record<string, number>
}

export interface ControllerBind {
  action: string
  button: string
}

export interface ControllerSettings {
  deadzone: number
  activeProfile: string
  defaultBinds: Record<string, Record<string, string>>
}

export interface ControllerType {
  type: string
  name: string
  supportsRumble: boolean
}

// Hook return type
interface UseControllerReturn {
  // Connected controllers
  controllers: ControllerInfo[]
  // Current state of a specific controller
  getState: (index: number) => Promise<ControllerState | null>
  // Settings
  settings: ControllerSettings | null
  // Supported controller types
  types: ControllerType[]
  // Loading states
  isLoading: boolean
  error: string | null
  // Actions
  setDeadzone: (value: number) => Promise<void>
  setProfile: (profile: string) => Promise<void>
  setControllerBinds: (controllerId: string, binds: Record<string, string>) => Promise<void>
  setTypeBinds: (controllerType: string, binds: Record<string, string>) => Promise<void>
  resetControllerBinds: (controllerId: string) => Promise<void>
  getControllerBinds: (controllerId: string) => Promise<Record<string, string> | null>
  rumble: (index: number, weakMagnitude?: number, strongMagnitude?: number, duration?: number) => Promise<void>
  // Polling control
  startPolling: () => void
  stopPolling: () => void
  pollingActive: boolean
  // Event callbacks
  onConnect: (callback: (controller: ControllerInfo) => void) => () => void
  onDisconnect: (callback: (index: number) => void) => () => void
}

/**
 * Hook for controller support (SDL2/XInput/DInput)
 * Provides access to connected controllers, state polling, and bind remapping
 * Uses the browser's Gamepad API to detect controllers and sends data to main process
 */
export function useController(): UseControllerReturn {
  const [controllers, setControllers] = useState<ControllerInfo[]>([])
  const [settings, setSettings] = useState<ControllerSettings | null>(null)
  const [types, setTypes] = useState<ControllerType[]>([])
  const [isLoading, setIsLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const pollIntervalRef = useRef<ReturnType<typeof setInterval> | null>(null)
  const [pollingActive, setPollingActive] = useState(false)
  const connectCallbacksRef = useRef<Set<(controller: ControllerInfo) => void>>(new Set())
  const disconnectCallbacksRef = useRef<Set<(index: number) => void>>(new Set())

  // Poll Gamepad API and send data to main process
  const pollGamepads = useCallback(() => {
    if (!window.ucController) {
      console.log('[Controller] ucController API not available yet')
      return
    }
    
    try {
      // Use the standard Gamepad API
      const gamepads = navigator.getGamepads ? navigator.getGamepads() : null
      if (!gamepads) return
      
      // Convert to array-like structure for sending
      const gamepadData: any[] = []
      for (const gp of gamepads) {
        if (gp) {
          gamepadData.push({
            index: gp.index,
            id: gp.id,
            connected: gp.connected,
            timestamp: gp.timestamp,
            // Serialize buttons
            buttons: gp.buttons.map((btn: any) => ({
              pressed: btn.pressed,
              value: btn.value,
              touched: btn.touched
            })),
            // Serialize axes
            axes: Array.from(gp.axes)
          })
        }
      }
      
      // Send to main process
      window.ucController.sendGamepadData(gamepadData)
    } catch (err) {
      console.error('[Controller] Gamepad poll error:', err)
    }
  }, [])

  // Start polling for gamepads
  const startPolling = useCallback(() => {
    if (pollIntervalRef.current || !window.ucController) return
    
    setPollingActive(true)
    // Poll at 60Hz (every ~16ms) for smooth input
    pollGamepads()
    pollIntervalRef.current = setInterval(pollGamepads, 16)
  }, [pollGamepads])

  // Stop polling
  const stopPolling = useCallback(() => {
    if (pollIntervalRef.current) {
      clearInterval(pollIntervalRef.current)
      pollIntervalRef.current = null
    }
    setPollingActive(false)
  }, [])

  // Initialize controller system
  useEffect(() => {
    const init = async () => {
      try {
        setIsLoading(true)
        setError(null)

        // Get supported controller types
        if (window.ucController) {
          const typesResult = await window.ucController.getControllerTypes()
          if (typesResult.ok) {
            setTypes(typesResult.types)
          }

          // Get current settings
          const settingsResult = await window.ucController.getSettings()
          if (settingsResult.ok) {
            setSettings({
              deadzone: settingsResult.deadzone,
              activeProfile: settingsResult.activeProfile,
              defaultBinds: settingsResult.defaultBinds
            })
          }

          // Get initial controller list
          const listResult = await window.ucController.listControllers()
          if (listResult.ok) {
            setControllers(listResult.controllers)
          }
        }
      } catch (err) {
        setError(err instanceof Error ? err.message : 'Failed to initialize controller system')
      } finally {
        setIsLoading(false)
      }
    }

    init()
  }, [])

  // Set up event listeners for controller connection/disconnection
  useEffect(() => {
    if (!window.ucController) return

    const unsubConnect = window.ucController.onConnected((data) => {
      // Refresh controller list
      window.ucController?.listControllers().then(result => {
        if (result.ok) {
          setControllers(result.controllers)
        }
      })
      
      // Notify callbacks
      connectCallbacksRef.current.forEach(callback => {
        callback({
          index: data.index,
          id: data.id,
          type: data.type,
          model: data.model,
          name: data.name,
          hasCustomBinds: false
        })
      })
    })

    const unsubDisconnect = window.ucController.onDisconnected((data) => {
      // Refresh controller list
      window.ucController?.listControllers().then(result => {
        if (result.ok) {
          setControllers(result.controllers)
        }
      })
      
      // Notify callbacks
      disconnectCallbacksRef.current.forEach(callback => {
        callback(data.index)
      })
    })

    // Also set up rumble handler
    const unsubRumble = window.ucController.onRumble((data) => {
      // Handle rumble in renderer using Gamepad API
      const gamepads = navigator.getGamepads ? navigator.getGamepads() : null
      if (gamepads) {
        const gamepad = gamepads[data.index]
        if (gamepad && gamepad.vibrationActuator) {
          gamepad.vibrationActuator.playEffect('dual-rumble', {
            startDelay: 0,
            duration: data.duration,
            weakMagnitude: data.weakMagnitude,
            strongMagnitude: data.strongMagnitude
          }).catch(err => console.error('[Controller] Rumble error:', err))
        }
      }
    })

    return () => {
      unsubConnect()
      unsubDisconnect()
      unsubRumble()
    }
  }, [])

  // Auto-start polling when hook is used
  useEffect(() => {
    // Wait for ucController API to be available before starting polling
    const checkAndStart = () => {
      if (window.ucController) {
        console.log('[Controller] ucController API available, starting polling')
        startPolling()
      } else {
        // Retry after a short delay if API not ready
        setTimeout(checkAndStart, 100)
      }
    }
    
    checkAndStart()
    
    // Also try polling when window gains focus (helps detect controllers that were connected when app wasn't focused)
    const handleFocus = () => {
      console.log('[Controller] Window focused, triggering poll')
      pollGamepads()
    }
    window.addEventListener('focus', handleFocus)
    
    return () => {
      stopPolling()
      window.removeEventListener('focus', handleFocus)
    }
  }, [startPolling, stopPolling, pollGamepads])

  // Get state for a specific controller
  const getState = useCallback(async (index: number): Promise<ControllerState | null> => {
    if (!window.ucController) return null
    
    const result = await window.ucController.getControllerState(index)
    if (result.ok && result.state) {
      return result.state
    }
    return null
  }, [])

  // Set deadzone
  const setDeadzone = useCallback(async (value: number) => {
    if (!window.ucController) return
    
    const result = await window.ucController.setDeadzone(value)
    if (result.ok && settings) {
      setSettings({ ...settings, deadzone: result.deadzone || value })
    }
  }, [settings])

  // Set active profile
  const setProfile = useCallback(async (profile: string) => {
    if (!window.ucController) return
    
    const result = await window.ucController.setProfile(profile)
    if (result.ok && settings) {
      setSettings({ ...settings, activeProfile: result.profile || profile })
    }
  }, [settings])

  // Set controller-specific binds
  const setControllerBinds = useCallback(async (controllerId: string, binds: Record<string, string>) => {
    if (!window.ucController) return
    
    const result = await window.ucController.setControllerBinds(controllerId, binds)
    if (result.ok) {
      // Refresh controller list to update hasCustomBinds flag
      const listResult = await window.ucController.listControllers()
      if (listResult.ok) {
        setControllers(listResult.controllers)
      }
    }
  }, [])

  // Set controller type binds
  const setTypeBinds = useCallback(async (controllerType: string, binds: Record<string, string>) => {
    if (!window.ucController) return
    
    await window.ucController.setTypeBinds(controllerType, binds)
  }, [])

  // Reset binds to default
  const resetControllerBinds = useCallback(async (controllerId: string) => {
    if (!window.ucController) return
    
    await window.ucController.resetControllerBinds(controllerId)
    
    // Refresh controller list
    const listResult = await window.ucController.listControllers()
    if (listResult.ok) {
      setControllers(listResult.controllers)
    }
  }, [])

  // Get controller binds
  const getControllerBinds = useCallback(async (controllerId: string): Promise<Record<string, string> | null> => {
    if (!window.ucController) return null
    
    const result = await window.ucController.getControllerBinds(controllerId)
    return result.binds
  }, [])

  // Trigger controller rumble
  const rumble = useCallback(async (
    index: number,
    weakMagnitude: number = 0.5,
    strongMagnitude: number = 0.5,
    duration: number = 500
  ) => {
    if (!window.ucController) return
    
    await window.ucController.rumble(index, weakMagnitude, strongMagnitude, duration)
  }, [])

  // Register connect callback
  const onConnect = useCallback((callback: (controller: ControllerInfo) => void) => {
    connectCallbacksRef.current.add(callback)
    return () => {
      connectCallbacksRef.current.delete(callback)
    }
  }, [])

  // Register disconnect callback
  const onDisconnect = useCallback((callback: (index: number) => void) => {
    disconnectCallbacksRef.current.add(callback)
    return () => {
      disconnectCallbacksRef.current.delete(callback)
    }
  }, [])

  // Cleanup on unmount
  useEffect(() => {
    return () => {
      if (pollIntervalRef.current) {
        clearInterval(pollIntervalRef.current)
      }
    }
  }, [])

  return {
    controllers,
    getState,
    settings,
    types,
    isLoading,
    error,
    setDeadzone,
    setProfile,
    setControllerBinds,
    setTypeBinds,
    resetControllerBinds,
    getControllerBinds,
    rumble,
    startPolling,
    stopPolling,
    pollingActive,
    onConnect,
    onDisconnect
  }
}

/**
 * Hook for real-time controller input polling
 * Use this for games that need real-time controller input
 */
export function useControllerInput(controllerIndex: number, enabled: boolean = true) {
  const [buttons, setButtons] = useState<Record<string, ControllerButtonState>>({})
  const [axes, setAxes] = useState<Record<string, number>>({})
  const [isConnected, setIsConnected] = useState(false)
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null)

  useEffect(() => {
    if (!enabled || !window.ucController) {
      setIsConnected(false)
      return
    }

    const poll = async () => {
      const result = await window.ucController!.getControllerState(controllerIndex)
      if (result.ok && result.state) {
        setButtons(result.state.buttons)
        setAxes(result.state.axes)
        setIsConnected(result.state.connected)
      } else {
        setIsConnected(false)
      }
    }

    // Poll at 60Hz for smooth input
    pollRef.current = setInterval(poll, 16)

    return () => {
      if (pollRef.current) {
        clearInterval(pollRef.current)
      }
    }
  }, [controllerIndex, enabled])

  return { buttons, axes, isConnected }
}

/**
 * Hook for detecting if a specific button/action is pressed
 */
export function useControllerButton(
  controllerIndex: number,
  buttonName: string,
  enabled: boolean = true
) {
  const [isPressed, setIsPressed] = useState(false)
  const { buttons } = useControllerInput(controllerIndex, enabled)

  useEffect(() => {
    const button = buttons[buttonName]
    setIsPressed(button?.pressed || false)
  }, [buttons, buttonName])

  return isPressed
}

export default useController
