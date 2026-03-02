import { useEffect, useState, useCallback } from "react"
import { Gamepad, Trash2, Save, AlertCircle, CheckCircle2 } from "lucide-react"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Slider } from "@/components/ui/slider"
import { Switch } from "@/components/ui/switch"
import { Badge } from "@/components/ui/badge"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"

interface ControllerDevice {
  index: number
  id: string
  type: string
  model: string
  name: string
  hasCustomBinds: boolean
}

const DEFAULT_BINDS: Record<string, string> = {
  buttonA: "a",
  buttonB: "b",
  buttonX: "x",
  buttonY: "y",
  leftBumper: "lb",
  rightBumper: "rb",
  leftTrigger: "lt",
  rightTrigger: "rt",
  leftStick: "ls",
  rightStick: "rs",
  dpadUp: "dup",
  dpadDown: "ddown",
  dpadLeft: "dleft",
  dpadRight: "dright",
  start: "start",
  back: "back"
}

const CONTROLLER_TYPES = [
  { id: "xbox", name: "Xbox", icon: "🎮" },
  { id: "playstation", name: "PlayStation", icon: "🎯" },
  { id: "nintendo", name: "Nintendo", icon: "🍄" },
  { id: "generic", name: "Generic", icon: "🕹️" }
]

const BUTTON_LABELS: Record<string, string> = {
  a: "A (South)",
  b: "B (East)",
  x: "X (West)",
  y: "Y (North)",
  lb: "Left Bumper",
  rb: "Right Bumper",
  lt: "Left Trigger",
  rt: "Right Trigger",
  ls: "Left Stick Click",
  rs: "Right Stick Click",
  dup: "D-Pad Up",
  ddown: "D-Pad Down",
  dleft: "D-Pad Left",
  dright: "D-Pad Right",
  start: "Start/Menu",
  back: "Back/View"
}

export function ControllerSettingsPanel() {
  const [controllers, setControllers] = useState<ControllerDevice[]>([])
  const [selectedController, setSelectedController] = useState<number | null>(null)
  const [deadzone, setDeadzone] = useState(0.15)
  const [currentBinds, setCurrentBinds] = useState<Record<string, string>>(DEFAULT_BINDS)
  const [editingBind, setEditingBind] = useState<string | null>(null)
  const [pendingBind, setPendingBind] = useState<string | null>(null)
  const [saving, setSaving] = useState(false)
  const [feedback, setFeedback] = useState<{ type: 'success' | 'error'; message: string } | null>(null)
  const [showAdvanced, setShowAdvanced] = useState(false)

  // Load controller list and settings
  useEffect(() => {
    const loadData = async () => {
      try {
        // Get connected controllers from main process
        if (window.ucController?.listControllers) {
          const result = await window.ucController.listControllers()
          if (result?.ok) {
            setControllers(result.controllers || [])
          }
        }

        // Get saved settings
        if (window.ucController?.getSettings) {
          const result = await window.ucController.getSettings()
          if (result?.ok) {
            setDeadzone(result.deadzone ?? 0.15)
          }
        }
      } catch (err) {
        console.error("[Controller] Failed to load data:", err)
      }
    }

    loadData()

    // Set up event listeners for controller connections
    const handleConnected = (data: { index: number; id: string; type: string; model: string; name: string }) => {
      setControllers(prev => {
        const exists = prev.find(c => c.index === data.index)
        if (exists) return prev
        return [...prev, { ...data, hasCustomBinds: false }]
      })
    }

    const handleDisconnected = (data: { index: number }) => {
      setControllers(prev => prev.filter(c => c.index !== data.index))
      if (selectedController === data.index) {
        setSelectedController(null)
      }
    }

    const unsubConnect = window.ucController?.onConnected?.(handleConnected)
    const unsubDisconnect = window.ucController?.onDisconnected?.(handleDisconnected)

    // Poll for controller updates
    const pollInterval = setInterval(async () => {
      if (window.ucController?.listControllers) {
        const result = await window.ucController.listControllers()
        if (result?.ok) {
          setControllers(result.controllers || [])
        }
      }
    }, 2000)

    return () => {
      clearInterval(pollInterval)
      unsubConnect?.()
      unsubDisconnect?.()
    }
  }, [selectedController])

  // Load binds when controller is selected
  useEffect(() => {
    const loadBinds = async () => {
      if (selectedController === null) {
        setCurrentBinds(DEFAULT_BINDS)
        return
      }

      try {
        const result = await window.ucController?.getControllerBinds?.(String(selectedController))
        if (result?.ok && result.binds) {
          setCurrentBinds(result.binds)
        } else {
          setCurrentBinds(DEFAULT_BINDS)
        }
      } catch (err) {
        console.error("[Controller] Failed to load binds:", err)
        setCurrentBinds(DEFAULT_BINDS)
      }
    }

    loadBinds()
  }, [selectedController])

  const handleDeadzoneChange = useCallback(async (value: number[]) => {
    const newDeadzone = value[0] / 100
    setDeadzone(newDeadzone)

    try {
      await window.ucController?.setDeadzone?.(newDeadzone)
    } catch (err) {
      console.error("[Controller] Failed to set deadzone:", err)
    }
  }, [])

  const startBindRemap = useCallback((action: string) => {
    setEditingBind(action)
    setPendingBind(action)
  }, [])

  const saveBinds = useCallback(async () => {
    if (selectedController === null) return

    setSaving(true)
    setFeedback(null)

    try {
      const result = await window.ucController?.setControllerBinds?.(String(selectedController), currentBinds)
      if (result?.ok) {
        setFeedback({ type: 'success', message: 'Controller binds saved successfully!' })
        setTimeout(() => setFeedback(null), 3000)
      } else {
        setFeedback({ type: 'error', message: result?.error || 'Failed to save binds.' })
      }
    } catch (err) {
      console.error("[Controller] Failed to save binds:", err)
      setFeedback({ type: 'error', message: 'Failed to save binds.' })
    } finally {
      setSaving(false)
    }
  }, [selectedController, currentBinds])

  const resetBinds = useCallback(async () => {
    if (selectedController === null) return

    setCurrentBinds(DEFAULT_BINDS)
    
    try {
      const result = await window.ucController?.resetControllerBinds?.(String(selectedController))
      if (result?.ok) {
        setFeedback({ type: 'success', message: 'Binds reset to defaults.' })
        setTimeout(() => setFeedback(null), 3000)
      }
    } catch (err) {
      console.error("[Controller] Failed to reset binds:", err)
    }
  }, [selectedController])

  const testRumble = useCallback(async () => {
    if (selectedController === null) return

    try {
      await window.ucController?.rumble?.(selectedController, 0.5, 0.5, 300)
    } catch (err) {
      console.error("[Controller] Failed to test rumble:", err)
    }
  }, [selectedController])

  const getControllerType = (name: string): string => {
    const lower = name.toLowerCase()
    if (lower.includes('xbox') || lower.includes('controller')) return 'xbox'
    if (lower.includes('playstation') || lower.includes('dualshock') || lower.includes('dualsense')) return 'playstation'
    if (lower.includes('nintendo') || lower.includes('switch') || lower.includes('joy-con')) return 'nintendo'
    return 'generic'
  }

  const getControllerTypeInfo = (type: string) => {
    return CONTROLLER_TYPES.find(t => t.id === type) || CONTROLLER_TYPES[3]
  }

  const selectedControllerInfo = controllers.find(c => c.index === selectedController)

  return (
    <Card className="border-border/60">
      <CardHeader className="pb-4">
        <div className="flex items-center gap-3">
          <Gamepad className="h-5 w-5 text-primary" />
          <CardTitle className="text-lg">Controller</CardTitle>
          <Badge variant={controllers.length > 0 ? "default" : "secondary"} className="ml-auto">
            {controllers.length} {controllers.length === 1 ? 'controller' : 'controllers'} connected
          </Badge>
        </div>
        <p className="text-sm text-muted-foreground mt-1">
          Configure controller settings, deadzone, and button mappings. Works with Xbox, PlayStation, Nintendo, and generic controllers.
        </p>
      </CardHeader>
      <CardContent className="space-y-6">
        {/* Connected Controllers */}
        <div className="space-y-3">
          <label className="text-sm font-medium">Connected Controllers</label>
          {controllers.length === 0 ? (
            <div className="rounded-lg border border-dashed border-border/60 p-4 text-center text-sm text-muted-foreground">
              No controllers detected. Connect a controller and press any button to detect it.
            </div>
          ) : (
            <div className="grid gap-2 sm:grid-cols-2">
              {controllers.map((controller) => {
                const typeInfo = getControllerTypeInfo(getControllerType(controller.name))
                const isSelected = selectedController === controller.index
                return (
                  <button
                    key={controller.index}
                    onClick={() => setSelectedController(controller.index)}
                    className={`rounded-lg border p-3 text-left transition-all ${
                      isSelected
                        ? 'border-primary bg-primary/10'
                        : 'border-border/60 hover:border-primary/50'
                    }`}
                  >
                    <div className="flex items-center gap-2">
                      <span className="text-xl">{typeInfo.icon}</span>
                      <div className="flex-1 min-w-0">
                        <div className="text-sm font-medium truncate">{controller.name}</div>
                        <div className="text-xs text-muted-foreground">{typeInfo.name} • {controller.hasCustomBinds ? 'Custom binds' : 'Default'}</div>
                      </div>
                      <div className="h-2 w-2 rounded-full bg-emerald-500" />
                    </div>
                  </button>
                )
              })}
            </div>
          )}
        </div>

        {/* Deadzone Slider */}
        <div className="space-y-3">
          <div className="flex items-center justify-between">
            <label className="text-sm font-medium">Stick Deadzone</label>
            <span className="text-sm text-muted-foreground">{Math.round(deadzone * 100)}%</span>
          </div>
          <Slider
            value={[deadzone * 100]}
            onValueChange={handleDeadzoneChange}
            min={0}
            max={30}
            step={1}
            className="w-full"
          />
          <p className="text-xs text-muted-foreground">
            Ignore small stick movements below this threshold to prevent drift.
          </p>
        </div>

        {/* Selected Controller Actions */}
        {selectedController !== null && (
          <div className="rounded-lg border border-border/60 bg-card/50 p-4 space-y-4">
            <div className="flex items-center justify-between">
              <h3 className="text-sm font-semibold">
                Selected: {selectedControllerInfo?.name || `Controller ${selectedController}`}
              </h3>
              <div className="flex gap-2">
                <Button
                  variant="outline"
                  size="sm"
                  onClick={testRumble}
                >
                  Test Rumble
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  onClick={resetBinds}
                >
                  <Trash2 className="h-3 w-3 mr-1" />
                  Reset
                </Button>
                <Button
                  size="sm"
                  onClick={saveBinds}
                  disabled={saving}
                >
                  <Save className="h-3 w-3 mr-1" />
                  {saving ? 'Saving...' : 'Save'}
                </Button>
              </div>
            </div>

            {/* Feedback Message */}
            {feedback && (
              <Alert variant={feedback.type === 'success' ? 'default' : 'destructive'}>
                {feedback.type === 'success' ? (
                  <CheckCircle2 className="h-4 w-4" />
                ) : (
                  <AlertCircle className="h-4 w-4" />
                )}
                <AlertDescription>{feedback.message}</AlertDescription>
              </Alert>
            )}

            {/* Bind Remapping */}
            <div className="space-y-3">
              <div className="flex items-center justify-between">
                <label className="text-sm font-medium">Button Mappings</label>
                <button
                  onClick={() => setShowAdvanced(!showAdvanced)}
                  className="text-xs text-primary hover:underline"
                >
                  {showAdvanced ? 'Hide' : 'Show'} Advanced
                </button>
              </div>

              <div className="grid gap-2 sm:grid-cols-2 md:grid-cols-3">
                {Object.entries(currentBinds).map(([action, button]) => {
                  const isEditing = editingBind === action
                  return (
                    <button
                      key={action}
                      onClick={() => startBindRemap(action)}
                      className={`rounded-md border px-3 py-2 text-left transition-all ${
                        isEditing
                          ? 'border-primary bg-primary/20 ring-1 ring-primary'
                          : 'border-border/60 hover:border-primary/50'
                      }`}
                    >
                      <div className="text-xs text-muted-foreground capitalize">{action.replace(/([A-Z])/g, ' $1').trim()}</div>
                      <div className="text-sm font-medium">
                        {isEditing ? (
                          <span className="text-primary animate-pulse">Press a button...</span>
                        ) : (
                          BUTTON_LABELS[button || ''] || button || 'Unbound'
                        )}
                      </div>
                    </button>
                  )
                })}
              </div>

              {/* Pending bind indicator */}
              {pendingBind && (
                <div className="rounded-md bg-primary/10 border border-primary/30 p-3">
                  <p className="text-sm text-primary">
                    Waiting for input on "{pendingBind.replace(/([A-Z])/g, ' $1').trim()}"... Press any button on your controller.
                  </p>
                </div>
              )}
            </div>
          </div>
        )}

        {/* Supported APIs Info */}
        <div className="rounded-lg bg-muted/30 p-4">
          <h4 className="text-xs font-semibold text-muted-foreground uppercase tracking-wide mb-2">Supported APIs</h4>
          <div className="flex flex-wrap gap-2">
            <Badge variant="outline" className="bg-background">SDL2</Badge>
            <Badge variant="outline" className="bg-background">XInput</Badge>
            <Badge variant="outline" className="bg-background">DirectInput</Badge>
            <Badge variant="outline" className="bg-background">Windows.Gaming.Input</Badge>
          </div>
          <p className="text-xs text-muted-foreground mt-2">
            Uses the browser's Gamepad API which supports all controller types on Windows through XInput and DirectInput.
          </p>
        </div>
      </CardContent>
    </Card>
  )
}
