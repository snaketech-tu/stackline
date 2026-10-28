<script lang="ts">
  import { app, type Theme } from '../lib/state.svelte'
  import { api, post } from '../lib/api'
  import { ago, bytes } from '../lib/format'
  import type { Settings, StoreStats } from '../lib/types'
  import Icon from '../components/Icon.svelte'

  let stats = $state<StoreStats | null>(null)
  let confirmClear = $state(false)

  async function loadStats() {
    stats = await api<StoreStats>('/store').catch(() => null)
  }
  $effect(() => {
    loadStats()
  })

  // "Saved" shows next to the control that changed, instead of a toast for every change.
  let saved = $state<string | null>(null)
  let savedTimer: ReturnType<typeof setTimeout> | undefined
  async function set<K extends keyof Settings>(key: K, value: Settings[K]) {
    if (!app.settings) return
    try {
      await app.saveSettings({ ...app.settings, [key]: value })
      saved = key
      clearTimeout(savedTimer)
      savedTimer = setTimeout(() => (saved = null), 1600)
    } catch (e) {
      app.notify(app.describe(e), 'err')
    }
  }

  async function clear() {
    try {
      await post('/store/clear', {})
      confirmClear = false
      app.notify('Stored logs cleared. swarmdash fetches recent lines again.')
      loadStats()
    } catch (e) {
      app.notify(app.describe(e), 'err')
    }
  }

  async function check() {
    try {
      await app.checkUpdate()
      if (app.update?.releases.length) app.drawer = { kind: 'update' }
      else if (app.update?.phase === 'upToDate') app.notify(`swarmdash ${app.update.current} is the newest version.`)
    } catch (e) {
      app.notify(app.describe(e), 'err')
    }
  }

  // Alerts: the desktop app shows native notifications; in a browser the page asks for permission.
  // The desktop app shows system notifications; known from the start, unlike the alert list.
  let native = $derived(!!app.meta?.desktop || app.alertsNative)
  let permission = $state(typeof Notification === 'undefined' ? 'unsupported' : Notification.permission)
  async function askPermission() {
    if (typeof Notification === 'undefined' || native) return
    try {
      permission = await Notification.requestPermission()
    } catch {
      permission = Notification.permission
    }
  }
  async function setAlerts(on: boolean) {
    if (on) await askPermission()
    await set('alertEnabled', on)
  }
  async function testAlert() {
    try {
      const r = await post<{ native: boolean }>('/alerts/test', {})
      if (r.native) return app.notify('Test notification sent.')
      await askPermission()
      if (permission !== 'granted') return app.notify('The browser blocks notifications for this page. Allow them in the site settings.', 'err')
      new Notification('Errors on your server', { body: `This is how swarmdash tells you about ${s!.alertThreshold} or more errors in ${s!.alertWindowMins} minutes.` })
    } catch (e) {
      app.notify(app.describe(e), 'err')
    }
  }

  const num = (e: Event) => Number((e.target as HTMLSelectElement).value)
  const on = (e: Event) => (e.target as HTMLInputElement).checked
  const THEMES: [Theme, string][] = [['system', 'System'], ['light', 'Light'], ['dark', 'Dark']]
  const opts = (list: number[], current: number) => (list.includes(current) ? list : [...list, current].sort((a, b) => a - b))
  let s = $derived(app.settings)
  let u = $derived(app.update)
</script>

{#if s}
  <section class="panel">
    <div class="ph"><h2>Appearance</h2></div>
    <div class="set">
      <span class="l">Theme</span>
      <span class="c">
        <div class="seg" role="group" aria-label="Theme">
          {#each THEMES as [t, label] (t)}<button aria-pressed={app.theme === t} onclick={() => app.setTheme(t)}>{label}</button>{/each}
        </div>
      </span>
      <span class="d">Follows your system until you pick light or dark. The button in the top bar switches it too.</span>
    </div>
  </section>

  <section class="panel">
    <div class="ph"><h2>Alerts</h2></div>
    <div class="set">
      <label class="l" for="pref-alertEnabled">Notify me when the error rate is high</label>
      <span class="c">
        <span class="saved" class:on={saved === 'alertEnabled'}>Saved</span>
        <span class="switch"><input type="checkbox" id="pref-alertEnabled" checked={s.alertEnabled} onchange={(e) => setAlerts(on(e))} /><i></i></span>
      </span>
      <span class="d">
        Counts the errors each connection logged and sends one notification when the count reaches the threshold, then waits until it drops again.
        Covers every connection you opened since swarmdash started, and only reads the logs stored on this computer.
        {#if !native}
          {#if permission === 'denied'}<b style="color:var(--err)">This browser blocks notifications for swarmdash. Allow them in the site settings; until then alerts only show inside the page.</b>
          {:else if permission === 'unsupported'}This browser cannot show notifications, so alerts show inside the page.
          {:else}Notifications show while swarmdash is open in a browser tab.{/if}
        {/if}
      </span>
    </div>
    {#if s.alertEnabled}
      {@render select('alertThreshold', 'Notify at', 'Errors in the window that trigger the notification.', opts([5, 10, 20, 50, 100, 250, 500, 1000], s.alertThreshold), (n) => `${n} errors`)}
      {@render select('alertWindowMins', 'Count errors over the last', '', opts([5, 10, 15], s.alertWindowMins), (m) => `${m} minutes`)}
      <div class="set">
        <span class="l">Test</span>
        <span class="c"><button class="btn" onclick={testAlert}><Icon name="bell" size={14} />Send a test notification</button></span>
        <span class="d">
          {#each app.alerts as a (a.conn)}
            <span style="display:block">{a.name}: {a.count} {a.count === 1 ? 'error' : 'errors'} in the last {a.windowMins} minutes{#if a.active}{' · '}<b style="color:var(--err)">over the threshold</b>{/if}</span>
          {:else}
            No connection checked yet.
          {/each}
        </span>
      </div>
    {/if}
  </section>

  <section class="panel">
    <div class="ph"><h2>Log collection</h2><span class="pill ok" style="font-size:11.5px">read-only</span></div>
    {@render select('retentionDays', 'Keep logs for', 'Older lines are deleted from this computer. The server is never touched.', opts([3, 7, 14, 30, 90], s.retentionDays), (d) => `${d} days`)}
    {@render select('maxStoreMb', 'Max local storage', 'When the store is full, the oldest lines go first.', opts([1024, 2048, 5120, 10240, 20480], s.maxStoreMb), (mb) => (mb >= 1024 ? mb / 1024 + ' GB' : mb + ' MB'))}
    {@render select('logPollSecs', 'Fetch new log lines every', 'After the first load swarmdash only asks for lines newer than the last one it saw, per replica.', opts([2, 3, 5, 15, 60], s.logPollSecs), (v) => `${v} s`)}
    {@render select('refreshSecs', 'Refresh containers and services every', '', opts([5, 10, 30, 60], s.refreshSecs), (v) => `${v} s`)}
    {@render toggle('collectStats', 'Read CPU and memory per container', 'One extra read-only stats call per container on every refresh.')}
    <div class="pb hint" style="border-top:1px solid var(--line)">Logs are read separately for every replica, so a multi-line stack trace from one replica never gets mixed with another. Multi-line entries from Java, .NET, Python, Node.js, PostgreSQL and MariaDB are joined automatically, and JSON logs (ECS, Logstash, Serilog, pino, zap and similar) are read field by field.</div>
  </section>

  <section class="panel">
    <div class="ph"><h2>Updates</h2></div>
    <div class="set">
      <span class="l">Version {app.meta?.version}</span>
      <span class="c"><button class="btn" onclick={check} disabled={u?.phase === 'checking'}>{u?.phase === 'checking' ? 'Checking…' : 'Check now'}</button></span>
      <span class="d">
        {#if !u}–
        {:else if u.releases.length}<button class="linkish" onclick={() => (app.drawer = { kind: 'update' })}>{u.releases[0].version} is available</button>
        {:else if u.phase === 'checking'}Checking…
        {:else if u.phase === 'failed'}{u.error}
        {:else if u.checkedAt}Up to date · checked {ago(u.checkedAt, app.now)}
        {:else}Not checked yet{/if}
      </span>
    </div>
    {@render toggle('checkUpdates', 'Check for a new version when swarmdash starts', 'Asks GitHub for the list of releases. Nothing is downloaded until you choose to update.')}
  </section>

  <section class="panel">
    <div class="ph"><h2>Local store</h2></div>
    <div class="set">
      <span class="l">{stats ? `${bytes(stats.bytes)} of ${bytes(s.maxStoreMb * 1024 * 1024)} · ${stats.entries.toLocaleString()} entries` : '–'}{#if stats?.oldest} · oldest {ago(stats.oldest / 1000, app.now)}{/if}</span>
      <span class="c">
        {#if confirmClear}
          <span class="confirm">Delete all stored logs?
            <button class="btn danger" onclick={clear}>Delete</button><button class="btn ghost" onclick={() => (confirmClear = false)}>Keep</button></span>
        {:else}
          <button class="btn" onclick={() => (confirmClear = true)}>Clear stored logs…</button>
        {/if}
      </span>
      <span class="d mono" style="font-size:11.5px;word-break:break-all">{app.meta?.storePath}</span>
    </div>
  </section>
{:else}
  <div class="center-empty"><span class="spinner"></span></div>
{/if}

{#snippet select(key: 'retentionDays' | 'maxStoreMb' | 'logPollSecs' | 'refreshSecs' | 'alertThreshold' | 'alertWindowMins', label: string, desc: string, list: number[], fmt: (v: number) => string)}
  <div class="set">
    <label class="l" for="pref-{key}">{label}</label>
    <span class="c">
      <span class="saved" class:on={saved === key}>Saved</span>
      <select id="pref-{key}" value={s![key]} onchange={(e) => set(key, num(e))}>
        {#each list as v (v)}<option value={v}>{fmt(v)}</option>{/each}
      </select>
    </span>
    {#if desc}<span class="d">{desc}</span>{/if}
  </div>
{/snippet}

{#snippet toggle(key: 'collectStats' | 'checkUpdates', label: string, desc: string)}
  <div class="set">
    <label class="l" for="pref-{key}">{label}</label>
    <span class="c">
      <span class="saved" class:on={saved === key}>Saved</span>
      <span class="switch"><input type="checkbox" id="pref-{key}" checked={s![key]} onchange={(e) => set(key, on(e))} /><i></i></span>
    </span>
    <span class="d">{desc}</span>
  </div>
{/snippet}
