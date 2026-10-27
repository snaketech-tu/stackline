<script lang="ts">
  import { onMount } from 'svelte'
  import { app, VIEWS, type Theme } from './lib/state.svelte'
  import Icon from './components/Icon.svelte'
  import Overview from './views/Overview.svelte'
  import Nodes from './views/Nodes.svelte'
  import Stacks from './views/Stacks.svelte'
  import Logs from './views/Logs.svelte'
  import Connections from './views/Connections.svelte'
  import Prefs from './views/Prefs.svelte'
  import ServiceDrawer from './components/ServiceDrawer.svelte'
  import NodeDrawer from './components/NodeDrawer.svelte'
  import DownloadDrawer from './components/DownloadDrawer.svelte'
  import UpdateDrawer from './components/UpdateDrawer.svelte'

  onMount(() => {
    app.init()
  })

  let title = $derived(VIEWS.find(([v]) => v === app.view)?.[1] ?? '')
  let dot = $derived.by(() => {
    if (!app.conn || app.conn.status === 'connecting') return 'off'
    if (app.conn.status === 'error') return 'err'
    const bad = app.services.some((s) => s.desired > 0 && s.running < s.desired) || app.snap?.nodes.some((n) => n.state !== 'ready')
    return bad ? 'warn' : ''
  })
  let connLine = $derived.by(() => {
    const c = app.conn
    if (!c) return 'connecting…'
    if (c.status === 'connecting') return 'connecting…'
    if (c.status === 'error') return 'not connected'
    return `${c.endpoint ?? ''}${c.latencyMs !== null ? ` · ${c.latencyMs} ms` : ''}`
  })
  let latest = $derived(app.update?.releases[0])
  let showUpdate = $derived(app.updateAvailable && !!latest && (app.dismissed !== latest.version || app.update?.phase !== 'available'))

  function keydown(e: KeyboardEvent) {
    if (e.key === 'Escape' && app.drawer) app.drawer = null
  }
</script>

<svelte:window onkeydown={keydown} />

<div class="app">
  <aside class="side">
    <div class="brand">
      <svg width="26" height="26" viewBox="0 0 26 26" aria-hidden="true"><rect x="1" y="1" width="24" height="24" rx="6" fill="var(--accent)" /><g fill="var(--panel)"><rect x="6" y="12" width="4" height="4" rx="1" /><rect x="11" y="12" width="4" height="4" rx="1" /><rect x="16" y="12" width="4" height="4" rx="1" /><rect x="11" y="7" width="4" height="4" rx="1" /><rect x="6" y="17" width="14" height="2" rx="1" /></g></svg>
      <div>swarmdash<small>Read-only</small></div>
    </div>
    <nav class="nav">
      {#each VIEWS as [v, label] (v)}
        <a href="#{v}" aria-current={app.view === v ? 'page' : undefined}>
          <Icon name={v} /><span class="t">{label}</span>
          {#if v === 'logs' && app.errors?.lastHour}<span class="cnt hide-sm" title="Errors in the last hour">{app.errors.lastHour}</span>{/if}
        </a>
      {/each}
    </nav>
    <div class="foot">
      <span class="ro"><Icon name="lock" size={12} />Read-only mode</span>
      <span>Only GET calls reach the Docker API. Nothing here can change your containers.</span>
      {#if app.meta}
        <span class="mono" style="font-size:11px">
          v{app.meta.version}
          {#if app.updateAvailable && latest}· <button class="linkish" onclick={() => (app.drawer = { kind: 'update' })}>v{latest.version} available</button>{/if}
        </span>
      {/if}
    </div>
  </aside>
  <div class="main">
    <header class="top">
      <h1>{title}</h1>
      <label class="ctl" title="Switch connection">
        <span class="dot {dot}"></span>
        <select style="border:0;background:transparent;outline:0;cursor:pointer;font-weight:600" value={app.activeId}
          onchange={(e) => app.select((e.target as HTMLSelectElement).value)} aria-label="Connection">
          {#each app.connections as c (c.id)}<option value={c.id}>{c.name}</option>{/each}
        </select>
      </label>
      <span class="conn hide-sm">{connLine}</span>
      <span class="spacer"></span>
      {#if showUpdate && latest}
        <button class="ctl update-pill" onclick={() => (app.drawer = { kind: 'update' })}>
          <Icon name="sparkle" size={14} />
          {#if app.update?.phase === 'downloading' || app.update?.phase === 'installing'}Updating…{:else if app.update?.phase === 'restarting'}Restarting…{:else}Update to {latest.version}{/if}
        </button>
      {/if}
      <select class="ctl theme-select" aria-label="Theme" value={app.theme} onchange={(e) => app.setTheme((e.target as HTMLSelectElement).value as Theme)}>
        <option value="system">Theme: system</option><option value="light">Theme: light</option><option value="dark">Theme: dark</option>
      </select>
    </header>
    <main class="view">
      {#if app.fatal}
        <div class="center-empty"><b style="color:var(--fg)">{app.fatal}</b><button class="btn" onclick={() => location.reload()}>Try again</button></div>
      {:else if app.view === 'connections'}
        <Connections />
      {:else if app.view === 'prefs'}
        <Prefs />
      {:else if app.conn?.status === 'error' && !app.snap}
        <div class="err-banner"><b>Cannot reach {app.active?.name ?? 'this connection'}.</b><span>{app.conn.error}</span></div>
        <div class="status-line"><span class="spinner"></span>Retrying every few seconds. Check the connection under <a href="#connections">Connections</a>.</div>
      {:else if !app.snap}
        <div class="center-empty"><span class="spinner"></span>Connecting to {app.active?.name ?? '…'}</div>
      {:else}
        {#if app.conn?.status === 'error'}
          <div class="err-banner"><b>Connection lost.</b><span>{app.conn.error} Showing the last known state.</span></div>
        {/if}
        {#if app.view === 'overview'}<Overview />
        {:else if app.view === 'nodes'}<Nodes />
        {:else if app.view === 'stacks'}<Stacks />
        {:else if app.view === 'logs'}<Logs />{/if}
      {/if}
    </main>
  </div>
</div>

{#if app.drawer?.kind === 'service'}<ServiceDrawer key={app.drawer.key} />
{:else if app.drawer?.kind === 'node'}<NodeDrawer id={app.drawer.id} />
{:else if app.drawer?.kind === 'download'}<DownloadDrawer />
{:else if app.drawer?.kind === 'update'}<UpdateDrawer />{/if}

{#if app.toast}<div class="toast" class:err={app.toast.kind === 'err'} role="status">{app.toast.text}</div>{/if}
