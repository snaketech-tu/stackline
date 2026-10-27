// App-wide state: connections, the active connection's snapshot, settings,
// pins, update status and the current view. Components read it directly.

import { SvelteSet } from 'svelte/reactivity'
import { ApiError, api, post, put } from './api'
import type {
  ConnState, Connection, ErrorSummary, Level, Meta, ServiceRef, Settings, Snapshot, SnapshotResponse, UpdateState,
} from './types'

export type View = 'overview' | 'nodes' | 'stacks' | 'logs' | 'connections' | 'prefs'
export const VIEWS: [View, string][] = [
  ['overview', 'Overview'],
  ['nodes', 'Nodes'],
  ['stacks', 'Stacks & services'],
  ['logs', 'Logs'],
  ['connections', 'Connections'],
  ['prefs', 'Preferences'],
]

export type Drawer =
  | { kind: 'service'; key: string }
  | { kind: 'node'; id: string }
  | { kind: 'download' }
  | { kind: 'update' }
  | null

export type Theme = 'system' | 'light' | 'dark'
export type Preset = '15m' | '1h' | '3h' | '24h' | '7d'
export const PRESETS: Record<Preset, number> = { '15m': 15, '1h': 60, '3h': 180, '24h': 1440, '7d': 10080 }

function load(key: string): string | null {
  try {
    return localStorage.getItem(key)
  } catch {
    return null
  }
}
function save(key: string, v: string) {
  try {
    localStorage.setItem(key, v)
  } catch {
    // Storage blocked: the setting just isn't remembered.
  }
}

/** The pin key of a service: `stack/service`. */
export const pinKey = (s: { stack: string; name: string }) => `${s.stack}/${s.name}`

/** Filters of the logs view. Kept here so they survive switching views. */
class LogsState {
  levels = new SvelteSet<Level>(['ERROR', 'WARN', 'INFO', 'DEBUG'])
  stack = $state('all')
  service = $state('all')
  q = $state('')
  preset = $state<Preset | null>('3h')
  from = $state(Date.now() - 3 * 3600e3)
  to = $state(Date.now())
  live = $state(false)
  collapse = $state(true)
  openGroups = new SvelteSet<string>()
  openRows = new SvelteSet<number>()

  /** The time range to query: a preset ends now, a zoom is fixed. */
  window(): [number, number] {
    if (this.preset) {
      const to = Date.now()
      return [to - PRESETS[this.preset] * 60e3, to]
    }
    return [this.from, this.to]
  }

  /** Query parameters for the current filters. */
  params(): Record<string, string | number> {
    const [from, to] = this.window()
    const p: Record<string, string | number> = { from: Math.floor(from), to: Math.ceil(to), levels: [...this.levels].join(',') }
    if (this.q.trim()) p.q = this.q.trim()
    if (this.stack !== 'all') p.stack = this.stack
    if (this.service !== 'all') p.service = this.service
    return p
  }

  setPreset(p: Preset) {
    this.preset = p
    this.to = Date.now()
    this.from = this.to - PRESETS[p] * 60e3
  }

  zoom(from: number, to: number) {
    this.preset = null
    this.live = false
    this.from = from
    this.to = to
  }

  reset() {
    this.stack = 'all'
    this.service = 'all'
    this.q = ''
    this.openGroups.clear()
    this.openRows.clear()
    this.live = false
    this.setPreset('3h')
  }
}

class AppState {
  meta = $state<Meta | null>(null)
  connections = $state<Connection[]>([])
  activeId = $state(load('swarmdash-conn') ?? 'local')
  snap = $state<Snapshot | null>(null)
  conn = $state<ConnState | null>(null)
  pins = $state<string[]>([])
  errors = $state<ErrorSummary | null>(null)
  settings = $state<Settings | null>(null)
  update = $state<UpdateState | null>(null)
  view = $state<View>('overview')
  drawer = $state<Drawer>(null)
  theme = $state<Theme>((load('swarmdash-theme') as Theme) || 'system')
  toast = $state<{ text: string; kind: 'ok' | 'err' } | null>(null)
  /** Set when the API cannot be used at all (server gone, signed out). */
  fatal = $state<string | null>(null)
  now = $state(Date.now())
  /** The update banner was dismissed for this version. */
  dismissed = $state(load('swarmdash-update-dismissed') ?? '')
  logs = new LogsState()

  active = $derived(this.connections.find((c) => c.id === this.activeId))

  services: ServiceRef[] = $derived(
    (this.snap?.stacks ?? []).flatMap((st) => st.services.map((s) => ({ ...s, kind: st.kind, key: pinKey(s) }))),
  )

  pinnedServices: ServiceRef[] = $derived(
    this.pins.map((k) => this.services.find((s) => s.key === k)).filter((s): s is ServiceRef => !!s),
  )

  updateAvailable = $derived(
    !!this.update && this.update.releases.length > 0 && !['upToDate', 'idle', 'checking'].includes(this.update.phase),
  )

  #timer: ReturnType<typeof setTimeout> | null = null
  #toastTimer: ReturnType<typeof setTimeout> | null = null
  #seq = 0

  async init() {
    this.applyTheme(this.theme)
    this.route()
    window.addEventListener('hashchange', () => this.route())
    setInterval(() => (this.now = Date.now()), 5000)
    try {
      const [meta, connections, settings] = await Promise.all([
        api<Meta>('/meta'),
        api<Connection[]>('/connections'),
        api<Settings>('/settings'),
      ])
      this.meta = meta
      this.connections = connections
      this.settings = settings
      if (!connections.some((c) => c.id === this.activeId)) this.activeId = connections[0]?.id ?? 'local'
    } catch (e) {
      this.fatal = this.describe(e)
      return
    }
    this.loadUpdate()
    this.poll()
  }

  describe(e: unknown): string {
    if (e instanceof ApiError && e.status === 401)
      return 'This page is not signed in. Open swarmdash with the link it printed when it started.'
    if (e instanceof ApiError) return e.message
    return 'swarmdash is not responding. Is it still running?'
  }

  route() {
    const h = location.hash.slice(1) as View
    this.view = VIEWS.some(([v]) => v === h) ? h : 'overview'
  }

  go(view: View) {
    if (location.hash.slice(1) === view) this.view = view
    else location.hash = view
  }

  select(id: string) {
    if (id === this.activeId) return
    this.activeId = id
    save('swarmdash-conn', id)
    this.snap = null
    this.conn = null
    this.errors = null
    this.pins = []
    this.drawer = null
    this.logs.reset()
    this.poll()
  }

  /** Refreshes the active connection now and then on the configured interval. */
  async poll() {
    if (this.#timer) clearTimeout(this.#timer)
    const seq = ++this.#seq
    const id = this.activeId
    try {
      const [res, errors] = await Promise.all([
        api<SnapshotResponse>(`/c/${encodeURIComponent(id)}/snapshot`),
        api<ErrorSummary>(`/c/${encodeURIComponent(id)}/errors`).catch(() => null),
      ])
      if (seq !== this.#seq) return
      this.conn = res.state
      if (res.snapshot) this.snap = res.snapshot
      this.pins = res.pins
      if (errors) this.errors = errors
      this.fatal = null
    } catch (e) {
      if (seq !== this.#seq) return
      if (e instanceof ApiError && e.status === 404) {
        await this.reloadConnections()
        if (this.activeId !== id) return
      }
      if (!(e instanceof ApiError) || e.status === 401) this.fatal = this.describe(e)
    }
    const connecting = this.conn?.status !== 'connected'
    const every = connecting ? 1500 : Math.max(2, this.settings?.refreshSecs ?? 10) * 1000
    this.#timer = setTimeout(() => this.poll(), document.hidden ? Math.max(every, 30000) : every)
  }

  async reloadConnections() {
    this.connections = await api<Connection[]>('/connections')
    if (!this.connections.some((c) => c.id === this.activeId) && this.connections[0]) this.select(this.connections[0].id)
  }

  isPinned(key: string): boolean {
    return this.pins.includes(key)
  }

  async togglePin(key: string) {
    const before = this.pins
    const next = before.includes(key) ? before.filter((k) => k !== key) : [...before, key]
    this.pins = next
    try {
      this.pins = await put<string[]>(`/c/${encodeURIComponent(this.activeId)}/pins`, next)
    } catch (e) {
      this.pins = before
      this.notify(this.describe(e), 'err')
    }
  }

  setTheme(t: Theme) {
    this.theme = t
    save('swarmdash-theme', t)
    this.applyTheme(t)
    if (this.settings && this.settings.theme !== t) {
      const s = { ...this.settings, theme: t }
      put<Settings>('/settings', s).then((r) => (this.settings = r)).catch(() => {})
    }
  }

  applyTheme(t: Theme) {
    const r = document.documentElement
    if (t === 'system') delete r.dataset.theme
    else r.dataset.theme = t
  }

  async saveSettings(s: Settings) {
    this.settings = await put<Settings>('/settings', s)
  }

  notify(text: string, kind: 'ok' | 'err' = 'ok') {
    this.toast = { text, kind }
    if (this.#toastTimer) clearTimeout(this.#toastTimer)
    this.#toastTimer = setTimeout(() => (this.toast = null), kind === 'err' ? 6000 : 3500)
  }

  // ---- updates ----

  #updateTimer: ReturnType<typeof setTimeout> | null = null

  async loadUpdate(tries = 0) {
    if (this.#updateTimer) clearTimeout(this.#updateTimer)
    const installing = !!this.update && ['downloading', 'installing', 'restarting'].includes(this.update.phase)
    try {
      this.update = await api<UpdateState>('/update')
    } catch {
      // The server going away mid-install means it is restarting into the new version.
      if (installing) {
        this.update = { ...this.update!, phase: 'restarting' }
        this.waitForRestart()
      }
      return
    }
    const p = this.update.phase
    if (p === 'restarting') {
      this.waitForRestart()
      return
    }
    // The server checks a moment after it starts; pick the result up when it lands.
    const startup = p === 'idle' && !!this.settings?.checkUpdates && tries < 30
    const pending = p === 'checking' || p === 'downloading' || p === 'installing' || startup
    this.#updateTimer = setTimeout(() => this.loadUpdate(pending ? tries + 1 : 0), pending ? 700 : 30 * 60e3)
  }

  async checkUpdate() {
    this.update = await post<UpdateState>('/update/check', {})
  }

  async installUpdate() {
    this.update = await post<UpdateState>('/update/install', {})
    this.loadUpdate()
  }

  dismissUpdate() {
    const v = this.update?.releases[0]?.version ?? ''
    this.dismissed = v
    save('swarmdash-update-dismissed', v)
  }

  /** After an update the server restarts. Reload once the new version answers. */
  #waiting = false
  async waitForRestart() {
    if (this.#waiting) return
    this.#waiting = true
    const before = this.meta?.version
    for (let i = 0; i < 240; i++) {
      await new Promise((r) => setTimeout(r, 500))
      try {
        const m = await api<Meta>('/meta')
        if (m.version !== before) {
          location.reload()
          return
        }
      } catch {
        // Still restarting.
      }
    }
    this.notify('swarmdash did not come back after the update. Start it again by hand.', 'err')
  }

  /** Opens a link in the system browser. The desktop window cannot open new tabs itself. */
  open(url: string) {
    if (this.meta?.desktop) post('/open', { url }).catch((e) => this.notify(this.describe(e), 'err'))
    else window.open(url, '_blank', 'noopener')
  }
}

export const app = new AppState()
