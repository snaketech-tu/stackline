// App-wide state: connections, the active connection's snapshot, settings,
// pins, update status and the current view. Components read it directly.

import { untrack } from 'svelte'
import { SvelteSet } from 'svelte/reactivity'
import { ApiError, api, post, put, type LogParams } from './api'
import { beforeEvent } from './events'
import type {
  Alert, AlertsResponse, Collect, ConnState, Connection, DockerEvent, ErrorSummary, Level, Meta, ServiceRef, Settings, Snapshot, SnapshotResponse,
  UpdateState,
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

export type ServiceTab = 'overview' | 'logs' | 'errors'
export type Drawer =
  | { kind: 'service'; key: string; tab: ServiceTab }
  | { kind: 'node'; id: string }
  | { kind: 'connection'; id: string | null }
  | { kind: 'update' }
  | null

export type Theme = 'system' | 'light' | 'dark'
export type Preset = 'auto' | '15m' | '1h' | '3h' | '24h' | '7d'
/** Minutes per fixed preset. `auto` starts at the oldest stored line instead. */
export const PRESETS: Record<Exclude<Preset, 'auto'>, number> = { '15m': 15, '1h': 60, '3h': 180, '24h': 1440, '7d': 10080 }
export const LEVELS: Level[] = ['ERROR', 'WARN', 'INFO', 'DEBUG']
export type StackFilter = 'all' | 'problems' | 'pinned'

export function load(key: string): string | null {
  try {
    return localStorage.getItem(key)
  } catch {
    return null
  }
}
export function save(key: string, v: string) {
  try {
    localStorage.setItem(key, v)
  } catch {
    // Storage blocked: the setting just isn't remembered.
  }
}

/** The pin key of a service: `stack/service`. */
export const pinKey = (s: { stack: string; name: string }) => `${s.stack}/${s.name}`

/** How many lines a stream shows at first, how many "Show more" adds, and the most the store returns at once. */
export const LINES = { first: 300, step: 500, max: 5000 }

/** What every log stream has: the levels shown, the open entries and how many lines are listed. */
export class LogFilters {
  levels = new SvelteSet<Level>(LEVELS)
  openRows = new SvelteSet<number>()
  /** How many of the newest lines the stream shows. "Show more" raises it. */
  limit = $state(LINES.first)

  /** Click shows only that level; with `add` (Shift-click) it is switched on or off. */
  pickLevel(l: Level, add: boolean) {
    if (add) {
      if (this.levels.has(l)) this.levels.delete(l)
      else this.levels.add(l)
      if (!this.levels.size) this.allLevels()
      return
    }
    const solo = this.levels.size === 1 && this.levels.has(l)
    this.levels.clear()
    for (const x of solo ? LEVELS : [l]) this.levels.add(x)
  }

  allLevels() {
    LEVELS.forEach((l) => this.levels.add(l))
  }

  /** Fewer than all levels are shown. */
  get soloLevels(): boolean {
    return this.levels.size < LEVELS.length
  }

  toggleRow(id: number) {
    if (this.openRows.has(id)) this.openRows.delete(id)
    else this.openRows.add(id)
  }

  /** Shows more of the newest lines, up to what the store returns at once. */
  more() {
    this.limit = Math.min(LINES.max, this.limit + LINES.step)
  }

  /** Back to every level, nothing open, the first lines only. */
  resetFilters() {
    this.levels.clear()
    this.allLevels()
    this.openRows.clear()
    this.limit = LINES.first
  }
}

/** Filters of the logs view. Kept here so they survive switching views. */
class LogsState extends LogFilters {
  /** Picked sources: full service names, or `stack:<name>` for a whole stack. Any of them matches. */
  sources = new SvelteSet<string>()
  q = $state('')
  /** The error group (fingerprint) the stream is narrowed to. */
  group = $state<string | null>(null)
  preset = $state<Preset | null>('auto')
  from = $state(Date.now() - 3 * 3600e3)
  to = $state(Date.now())
  live = $state(false)
  collapse = $state(true)
  /** Error groups above the stream (it gets the full width) or beside it. */
  groupsAt = $state<'top' | 'side'>(load('swarmdash-groups-at') === 'side' ? 'side' : 'top')
  groupsOpen = $state(load('swarmdash-groups-open') !== '0')
  /** The stream fills the window. */
  maxi = $state(false)

  /** Where `auto` starts: the oldest stored line, within the retention, at least 15 minutes back. */
  autoFrom: () => number
  constructor(autoFrom: () => number) {
    super()
    this.autoFrom = autoFrom
  }

  /** The time range to query: a preset ends now, a zoom is fixed. */
  window(): [number, number] {
    if (this.preset) {
      const to = Date.now()
      return [this.preset === 'auto' ? Math.min(this.autoFrom(), to - 15 * 60e3) : to - PRESETS[this.preset] * 60e3, to]
    }
    return [this.from, this.to]
  }

  /** Query parameters for the current filters. */
  params(): LogParams {
    const [from, to] = this.window()
    const p: LogParams = { from: Math.floor(from), to: Math.ceil(to), levels: [...this.levels].join(',') }
    const q = [this.q.trim(), this.group ? `fp:${this.group}` : ''].filter(Boolean).join(' ')
    if (q) p.q = q
    const stacks = [...this.sources].filter((s) => s.startsWith('stack:')).map((s) => s.slice(6))
    const services = [...this.sources].filter((s) => !s.startsWith('stack:'))
    if (stacks.length) p.stack = stacks.join(',')
    if (services.length) p.service = services.join(',')
    return p
  }

  setGroupsAt(v: 'top' | 'side') {
    this.groupsAt = v
    save('swarmdash-groups-at', v)
  }

  toggleGroups() {
    this.groupsOpen = !this.groupsOpen
    save('swarmdash-groups-open', this.groupsOpen ? '1' : '0')
  }

  setPreset(p: Preset) {
    this.preset = p
    ;[this.from, this.to] = this.window()
  }

  zoom(from: number, to: number) {
    this.preset = null
    this.live = false
    this.from = from
    this.to = to
  }

  /**
   * The filters as the part of a link after `#`: `logs?c=…&t=1h&s=…`. With
   * `fixed`, a preset becomes the range it covers now, so the link shows the
   * same lines when it is opened later.
   */
  link(conn: string, fixed = false): string {
    const p = new URLSearchParams({ c: conn })
    if (!this.preset || fixed) {
      const [from, to] = this.window()
      p.set('t', `${Math.floor(from)}-${Math.ceil(to)}`)
    } else if (this.preset !== 'auto') p.set('t', this.preset)
    if (this.sources.size) p.set('s', [...this.sources].join(','))
    if (this.soloLevels) p.set('l', [...this.levels].join(','))
    if (this.q.trim()) p.set('q', this.q.trim())
    if (this.group) p.set('g', this.group)
    return 'logs?' + p.toString()
  }

  /** Shows what a link made by `link` describes. Values it does not know are left out. */
  applyLink(p: URLSearchParams) {
    this.reset()
    const t = p.get('t') ?? ''
    const range = /^(\d+)-(\d+)$/.exec(t)
    if (range && +range[1] < +range[2]) this.zoom(+range[1], +range[2])
    else if (t in PRESETS) this.setPreset(t as Preset)
    this.sources.clear()
    for (const src of (p.get('s') ?? '').split(',')) if (src) this.sources.add(src)
    const levels = (p.get('l') ?? '').split(',').filter((l): l is Level => (LEVELS as string[]).includes(l))
    if (levels.length) {
      this.levels.clear()
      levels.forEach((l) => this.levels.add(l))
    }
    this.q = p.get('q') ?? ''
    this.group = p.get('g') || null
  }

  /** Puts back the sources last picked on a connection. */
  restoreSources(conn: string) {
    this.sources.clear()
    try {
      for (const k of JSON.parse(load(`swarmdash-sources-${conn}`) ?? '[]')) if (typeof k === 'string') this.sources.add(k)
    } catch {
      // A damaged entry means no picked sources.
    }
  }

  toggleSource(key: string) {
    if (this.sources.has(key)) this.sources.delete(key)
    else this.sources.add(key)
  }

  /** Back to the default filters. The picked sources stay: they are the services someone follows. */
  reset() {
    this.resetFilters()
    this.q = ''
    this.group = null
    this.live = false
    this.maxi = false
    this.setPreset('auto')
  }
}

class AppState {
  meta = $state<Meta | null>(null)
  connections = $state<Connection[]>([])
  activeId = $state(load('swarmdash-conn') ?? 'local')
  /** Replaced as a whole on every refresh, so it needs no deep reactivity. */
  snap = $state.raw<Snapshot | null>(null)
  conn = $state<ConnState | null>(null)
  pins = $state<string[]>([])
  /** Which services of the active connection have their logs collected. */
  collect = $state<Collect>({ only: false, services: [] })
  errors = $state.raw<ErrorSummary | null>(null)
  settings = $state<Settings | null>(null)
  update = $state<UpdateState | null>(null)
  view = $state<View>('overview')
  drawer = $state<Drawer>(null)
  palette = $state(false)
  /** Filters of the stacks view, set from the overview too. */
  stackFilter = $state<StackFilter>((load('swarmdash-stack-filter') as StackFilter) || 'all')
  stackQ = $state(load('swarmdash-stack-q') ?? '')
  theme = $state<Theme>((load('swarmdash-theme') as Theme) || 'system')
  toast = $state<{ text: string; kind: 'ok' | 'err' } | null>(null)
  /** Set when the API cannot be used at all (server gone, signed out). */
  fatal = $state<string | null>(null)
  now = $state(Date.now())
  /** The update banner was dismissed for this version. */
  dismissed = $state(load('swarmdash-update-dismissed') ?? '')
  logs = new LogsState(() => {
    const now = Date.now()
    const keep = now - (this.settings?.retentionDays ?? 2) * 86400e3
    const oldest = this.errors?.oldest ? this.errors.oldest / 1000 : now - 3 * 3600e3
    return Math.max(keep, oldest)
  })

  /** Error-rate alerts of every opened connection. */
  alerts = $state<Alert[]>([])
  /** The desktop app shows alerts as native notifications; a browser tab shows them itself. */
  alertsNative = $state(false)

  active = $derived(this.connections.find((c) => c.id === this.activeId))
  activeAlert = $derived(this.alerts.find((a) => a.conn === this.activeId && a.active))

  services: ServiceRef[] = $derived(
    (this.snap?.stacks ?? []).flatMap((st) => st.services.map((s) => ({ ...s, kind: st.kind, key: pinKey(s) }))),
  )

  pinnedServices: ServiceRef[] = $derived(
    this.pins.map((k) => this.services.find((s) => s.key === k)).filter((s): s is ServiceRef => !!s),
  )

  updateAvailable = $derived(
    !!this.update && this.update.releases.length > 0 && !['upToDate', 'idle', 'checking'].includes(this.update.phase),
  )

  constructor() {
    this.logs.restoreSources(this.activeId)
    // Filters survive moving around the app and restarting it. Sources are kept per connection.
    $effect.root(() => {
      $effect(() => {
        const v = JSON.stringify([...this.logs.sources])
        save(`swarmdash-sources-${untrack(() => this.activeId)}`, v)
      })
      // The address of the Logs page follows its filters, so it can be bookmarked,
      // reloaded or copied. Replaced in place: filters are not steps of Back.
      $effect(() => {
        if (this.view !== 'logs') return
        const h = '#' + this.logs.link(this.activeId)
        if (location.hash !== h) history.replaceState(history.state, '', h)
      })
      $effect(() => {
        save('swarmdash-stack-filter', this.stackFilter)
        save('swarmdash-stack-q', this.stackQ)
      })
    })
  }

  /** The connection a link opened before the connections were loaded. */
  #linkConn: string | null = null
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
      if (!connections.some((c) => c.id === this.activeId)) {
        const missing = this.#linkConn
        this.activeId = connections.find((c) => c.id === load('swarmdash-conn'))?.id ?? connections[0]?.id ?? 'local'
        if (missing) this.notify(`This link is for the connection “${missing}”, which is not set up here. It shows ${this.active?.name ?? 'another one'} instead.`, 'err')
        else this.logs.restoreSources(this.activeId)
      } else if (this.#linkConn) save('swarmdash-conn', this.activeId)
      this.#linkConn = null
    } catch (e) {
      this.fatal = this.describe(e)
      return
    }
    this.loadUpdate()
    this.poll()
    this.loadAlerts()
    // Only while alerts are on: the server has nothing to say otherwise.
    setInterval(() => (this.settings?.alertEnabled ? this.loadAlerts() : (this.alerts = [])), 15000)
  }

  // ---- error-rate alerts ----

  /** The newest notification seen per connection; undefined until the first load. */
  #seen: Map<string, number> | undefined

  async loadAlerts() {
    let r: AlertsResponse
    try {
      r = await api<AlertsResponse>('/alerts')
    } catch {
      return
    }
    this.alerts = r.alerts
    this.alertsNative = r.native
    const first = !this.#seen
    const seen = (this.#seen ??= new Map())
    for (const a of r.alerts) {
      const was = seen.get(a.conn) ?? 0
      seen.set(a.conn, a.seq)
      // On the first load only alerts from the last minute are news.
      if (a.seq <= was || (first && (!a.notifiedAt || Date.now() - a.notifiedAt > 60e3))) continue
      this.announce(a)
    }
  }

  announce(a: Alert) {
    const text = `${a.count} ${a.count === 1 ? 'error' : 'errors'} in the last ${a.windowMins} minutes on ${a.name}.`
    this.notify(text, 'err')
    if (this.alertsNative || this.meta?.desktop || typeof Notification === 'undefined' || Notification.permission !== 'granted') return
    try {
      const body = `${text}${a.top ? `\nMost frequent: ${a.top}${a.topService ? ` (${a.topService})` : ''}` : ''}`
      const n = new Notification(`Errors on ${a.name}`, { body, tag: `swarmdash-${a.conn}` })
      n.onclick = () => {
        window.focus()
        this.openAlert(a)
        n.close()
      }
    } catch {
      // Some browsers only allow notifications from a service worker; the toast is shown anyway.
    }
  }

  /**
   * Shows the errors behind an alert: its connection, every service (the
   * alert counts them all, so picked sources would hide some), errors only,
   * over the alert's own window.
   */
  openAlert(a: Alert) {
    this.select(a.conn)
    this.showLogs([], ['ERROR'])
    this.logs.sources.clear()
    const now = Date.now()
    this.logs.zoom(now - a.windowMins * 60e3, now)
  }

  /**
   * The lines a replica printed in the 2 minutes before it stopped or
   * crashed: usually where the reason is.
   */
  showEvent(e: DockerEvent) {
    this.showLogs([e.service])
    const [from, to] = beforeEvent(e)
    this.logs.zoom(from, to)
    this.logs.q = `replica:${e.replica}`
  }

  /** The stored range of the active connection: at most `max` ms back, at least 15 minutes. */
  storedRange(max: number): { from: number; to: number } {
    const to = Date.now()
    const oldest = this.errors?.oldest ? this.errors.oldest / 1000 : 0
    return { from: Math.floor(Math.max(to - max, Math.min(oldest, to - 15 * 60e3))), to }
  }


  describe(e: unknown): string {
    if (e instanceof ApiError && e.status === 401)
      return 'This page is not signed in. Open swarmdash with the link it printed when it started.'
    if (e instanceof ApiError) return e.message
    return 'swarmdash is not responding. Is it still running?'
  }

  route() {
    const hash = location.hash.slice(1)
    const at = hash.indexOf('?')
    const h = (at < 0 ? hash : hash.slice(0, at)) as View
    this.view = VIEWS.some(([v]) => v === h) ? h : 'overview'
    if (this.view === 'logs' && at >= 0) this.openLink(new URLSearchParams(hash.slice(at + 1)))
  }

  /** Opens a log link: its connection, then its filters. */
  openLink(p: URLSearchParams) {
    const c = p.get('c')
    if (c && c !== this.activeId) {
      // While the connections are still loading, init checks this one.
      if (!this.connections.length) {
        this.activeId = c
        this.#linkConn = c
      } else if (this.connections.some((x) => x.id === c)) this.select(c)
      else this.notify(`This link is for the connection “${c}”, which is not set up here. It shows ${this.active?.name ?? 'this one'} instead.`, 'err')
    }
    this.logs.applyLink(p)
  }

  /** The address of the current Logs view, with its range fixed, for others to open. */
  logLink(): string {
    return `${location.origin}${location.pathname}#${this.logs.link(this.activeId, true)}`
  }

  go(view: View) {
    if (location.hash.slice(1).split('?')[0] === view) this.view = view
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
    this.collect = { only: false, services: [] }
    this.drawer = null
    this.logs.reset()
    this.logs.restoreSources(id)
    this.stackQ = ''
    this.stackFilter = 'all'
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
      if (!this.#collectBusy) this.collect = res.collect ?? { only: false, services: [] }
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

  /** True when the logs of this service (full name) are collected. */
  collects(name: string): boolean {
    return this.collect.services.includes(name) === this.collect.only
  }

  #collectBusy = 0

  async saveCollect(next: Collect) {
    const before = this.collect
    this.collect = next
    this.#collectBusy++
    try {
      this.collect = await put<Collect>(`/c/${encodeURIComponent(this.activeId)}/collect`, next)
    } catch (e) {
      this.collect = before
      this.notify(this.describe(e), 'err')
    } finally {
      this.#collectBusy--
    }
  }

  /** Switches log collection of these services (full names) on or off. */
  setCollects(names: string[], on: boolean) {
    const { only, services } = this.collect
    const listed = on === only
    const rest = services.filter((s) => !names.includes(s))
    this.saveCollect({ only, services: listed ? [...rest, ...names] : rest })
  }

  /**
   * Collect every service, or only chosen ones. Choosing starts from the
   * pinned services, since those are the ones someone follows.
   */
  setCollectOnly(only: boolean) {
    if (only === this.collect.only) return
    const services = only ? this.pinnedServices.map((s) => s.fullName).filter((n) => this.collects(n)) : []
    this.saveCollect({ only, services })
  }

  /** Starts deleting the stored lines of one service (full name) of the active connection. */
  async clearService(name: string) {
    await post(`/c/${encodeURIComponent(this.activeId)}/usage/clear`, { service: name })
    this.notify(`Deleting the stored lines of ${name}. This runs in the background; Preferences shows when it is done.`)
  }

  /** The theme button in the top bar: system → light → dark → system. */
  cycleTheme() {
    this.setTheme(({ system: 'light', light: 'dark', dark: 'system' } as const)[this.theme])
  }

  openService(key: string, tab: ServiceTab = 'overview') {
    this.drawer = { kind: 'service', key, tab }
  }

  /** Opens the logs view. Given sources (full service names or `stack:<name>`) replace the picked ones. */
  showLogs(sources: string[] = [], levels?: Level[]) {
    this.logs.reset()
    if (sources.length) {
      this.logs.sources.clear()
      for (const s of sources) this.logs.sources.add(s)
    }
    if (levels) {
      this.logs.levels.clear()
      levels.forEach((l) => this.logs.levels.add(l))
    }
    this.drawer = null
    this.go('logs')
  }

  /**
   * Opens the logs view narrowed to one error group. The picked sources stay
   * unless none of them can show it: then the view would be empty.
   */
  showGroup(fp: string, services: string[] = []) {
    const picked = [...this.logs.sources]
    const stackOf = (name: string) => this.services.find((s) => s.fullName === name)?.stack
    const shown = services.some((n) => picked.includes(n) || picked.includes(`stack:${stackOf(n)}`))
    this.showLogs()
    if (picked.length && services.length && !shown) this.logs.sources.clear()
    this.logs.group = fp
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

  async copy(text: string, done = 'Copied to the clipboard.') {
    try {
      await navigator.clipboard.writeText(text)
      this.notify(done)
    } catch {
      this.notify('The clipboard is not available here.', 'err')
    }
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
