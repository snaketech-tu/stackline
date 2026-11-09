// The log workspace at the bottom of the window: one tab per log source
// someone opened. The tabs keep following their lines while the rest of
// the app is used, and are remembered per connection.

import { load, save } from './storage'

export interface DockTab {
  id: string
  /** What the tab is named. */
  label: string
  /** A full service name, `stack:<name>` for a whole stack, or empty for every service. */
  source: string
  /** Search the tab starts with, such as `replica:2` or `node:web-1`. */
  q: string
}

const MIN = 160

export class DockState {
  tabs = $state<DockTab[]>([])
  active = $state<string | null>(null)
  /** Height in pixels of the open workspace. */
  height = $state(Math.max(MIN, Number(load('stackline-dock-height')) || 300))
  /** Only the tab strip shows. */
  min = $state(false)
  #conn = ''

  /** Puts back the tabs last open on a connection. */
  restore(conn: string) {
    this.#conn = conn
    this.tabs = []
    this.active = null
    try {
      const v = JSON.parse(load(`stackline-dock-${conn}`) ?? 'null')
      if (v && Array.isArray(v.tabs)) {
        this.tabs = v.tabs.filter((t: DockTab) => t && typeof t.id === 'string' && typeof t.source === 'string')
        this.active = this.tabs.some((t) => t.id === v.active) ? v.active : (this.tabs[0]?.id ?? null)
      }
    } catch {
      // A damaged entry means no tabs.
    }
  }

  #save() {
    save(`stackline-dock-${this.#conn}`, JSON.stringify({ tabs: this.tabs, active: this.active }))
  }

  /** Opens a tab for these lines, or shows the one already open for them. */
  open(source: string, label: string, q = '') {
    const id = `${source}|${q}`
    if (!this.tabs.some((t) => t.id === id)) this.tabs = [...this.tabs, { id, label, source, q }]
    this.active = id
    this.min = false
    this.#save()
  }

  close(id: string) {
    const i = this.tabs.findIndex((t) => t.id === id)
    if (i < 0) return
    this.tabs = this.tabs.filter((t) => t.id !== id)
    if (this.active === id) this.active = (this.tabs[i] ?? this.tabs[i - 1])?.id ?? null
    this.#save()
  }

  closeAll() {
    this.tabs = []
    this.active = null
    this.#save()
  }

  pick(id: string) {
    this.active = id
    this.min = false
    this.#save()
  }

  toggle() {
    if (this.tabs.length) this.min = !this.min
  }

  resize(h: number) {
    this.height = Math.max(MIN, Math.min(h, innerHeight - 220))
    save('stackline-dock-height', String(Math.round(this.height)))
  }
}
