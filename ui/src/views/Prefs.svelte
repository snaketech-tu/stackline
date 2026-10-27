<script lang="ts">
  import { app, type Theme } from '../lib/state.svelte'
  import { api, post } from '../lib/api'
  import { ago, bytes } from '../lib/format'
  import type { Settings, StoreStats } from '../lib/types'

  let stats = $state<StoreStats | null>(null)
  let confirmClear = $state(false)

  async function loadStats() {
    stats = await api<StoreStats>('/store').catch(() => null)
  }
  $effect(() => {
    loadStats()
  })

  async function set<K extends keyof Settings>(key: K, value: Settings[K]) {
    if (!app.settings) return
    try {
      await app.saveSettings({ ...app.settings, [key]: value })
      app.notify('Saved.')
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

  const num = (e: Event) => Number((e.target as HTMLSelectElement).value)
  const opts = (list: number[], current: number) => (list.includes(current) ? list : [...list, current].sort((a, b) => a - b))
  let s = $derived(app.settings)
  let u = $derived(app.update)
</script>

{#if s}
  <section class="panel">
    <div class="ph"><h2>Appearance</h2></div>
    <div class="pb form">
      <label>Theme
        <select value={app.theme} onchange={(e) => app.setTheme((e.target as HTMLSelectElement).value as Theme)}>
          <option value="system">Follow system</option><option value="light">Light</option><option value="dark">Dark</option>
        </select>
      </label>
    </div>
  </section>

  <section class="panel">
    <div class="ph"><h2>Log collection</h2><span class="pill ok" style="font-size:11.5px">read-only</span></div>
    <div class="pb form">
      <label>Keep logs for
        <select value={s.retentionDays} onchange={(e) => set('retentionDays', num(e))}>
          {#each opts([3, 7, 14, 30, 90], s.retentionDays) as d (d)}<option value={d}>{d} days</option>{/each}
        </select>
      </label>
      <label>Max local storage
        <select value={s.maxStoreMb} onchange={(e) => set('maxStoreMb', num(e))}>
          {#each opts([1024, 2048, 5120, 10240, 20480], s.maxStoreMb) as mb (mb)}<option value={mb}>{mb >= 1024 ? mb / 1024 + ' GB' : mb + ' MB'}</option>{/each}
        </select>
      </label>
      <label>Fetch new log lines every
        <select value={s.logPollSecs} onchange={(e) => set('logPollSecs', num(e))}>
          {#each opts([2, 3, 5, 15, 60], s.logPollSecs) as v (v)}<option value={v}>{v} s</option>{/each}
        </select>
      </label>
      <label>Refresh containers and services every
        <select value={s.refreshSecs} onchange={(e) => set('refreshSecs', num(e))}>
          {#each opts([5, 10, 30, 60], s.refreshSecs) as v (v)}<option value={v}>{v} s</option>{/each}
        </select>
      </label>
      <label class="toggle full" style="color:var(--fg)"><input type="checkbox" checked={s.collectStats} onchange={(e) => set('collectStats', (e.target as HTMLInputElement).checked)} />Read CPU and memory per container</label>
      <div class="full hint">Logs are read separately for every replica, so a multi-line stack trace from one replica never gets mixed with another. After the first load swarmdash only asks for lines newer than the last one it saw, which keeps the load on the server low. Multi-line entries from Java, .NET, Python, Node.js, PostgreSQL and MariaDB are joined automatically, and JSON logs (ECS, Logstash, Serilog, pino, zap and similar) are read field by field.</div>
    </div>
  </section>

  <section class="panel">
    <div class="ph"><h2>Updates</h2></div>
    <div class="pb" style="display:flex;gap:18px;flex-wrap:wrap;align-items:center">
      <dl class="kv">
        <dt>Version</dt><dd>{app.meta?.version}</dd>
        <dt>Status</dt>
        <dd>
          {#if !u}–
          {:else if u.releases.length}<button class="linkish" onclick={() => (app.drawer = { kind: 'update' })}>{u.releases[0].version} is available</button>
          {:else if u.phase === 'checking'}Checking…
          {:else if u.phase === 'failed'}{u.error}
          {:else if u.checkedAt}Up to date (checked {ago(u.checkedAt, app.now)})
          {:else}Not checked yet{/if}
        </dd>
      </dl>
      <span class="spacer"></span>
      <button class="btn" onclick={check} disabled={u?.phase === 'checking'}>Check now</button>
    </div>
    <div class="pb" style="padding-top:0">
      <label class="toggle" style="color:var(--fg)"><input type="checkbox" checked={s.checkUpdates} onchange={(e) => set('checkUpdates', (e.target as HTMLInputElement).checked)} />Check for a new version when swarmdash starts</label>
      <div class="hint" style="margin-top:6px">The check asks GitHub for the list of releases. Nothing is downloaded until you choose to update.</div>
    </div>
  </section>

  <section class="panel">
    <div class="ph"><h2>Local store</h2></div>
    <div class="pb" style="display:flex;gap:18px;flex-wrap:wrap;align-items:center">
      <dl class="kv">
        <dt>Size</dt><dd>{stats ? `${bytes(stats.bytes)} of ${bytes(s.maxStoreMb * 1024 * 1024)}` : '–'}</dd>
        <dt>Entries</dt><dd>{stats ? stats.entries.toLocaleString() : '–'}</dd>
        <dt>Oldest entry</dt><dd>{stats?.oldest ? ago(stats.oldest / 1000, app.now) : '–'}</dd>
        <dt>Location</dt><dd style="word-break:break-all">{app.meta?.storePath}</dd>
      </dl>
      <span class="spacer"></span>
      {#if confirmClear}
        <span class="confirm">Delete all stored logs?
          <button class="btn danger" onclick={clear}>Delete</button><button class="btn ghost" onclick={() => (confirmClear = false)}>Keep</button></span>
      {:else}
        <button class="btn" onclick={() => (confirmClear = true)}>Clear stored logs…</button>
      {/if}
    </div>
  </section>
{:else}
  <div class="center-empty"><span class="spinner"></span></div>
{/if}
