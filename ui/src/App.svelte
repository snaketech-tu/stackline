<script lang="ts">
  import { onMount } from 'svelte'
  import { app, VIEWS, type View } from './lib/state.svelte'
  import Icon from './components/Icon.svelte'
  import Overview from './views/Overview.svelte'
  import Nodes from './views/Nodes.svelte'
  import Stacks from './views/Stacks.svelte'
  import Logs from './views/Logs.svelte'
  import Connections from './views/Connections.svelte'
  import Prefs from './views/Prefs.svelte'
  import ServiceDrawer from './components/ServiceDrawer.svelte'
  import NodeDrawer from './components/NodeDrawer.svelte'
  import ConnectionDrawer from './components/ConnectionDrawer.svelte'
  import UpdateDrawer from './components/UpdateDrawer.svelte'
  import ConnMenu from './components/ConnMenu.svelte'
  import Palette from './components/Palette.svelte'

  onMount(() => {
    app.init()
  })

  // On a single node the overview's host card says what the nodes page would.
  let views = $derived(VIEWS.filter(([v]) => v !== 'nodes' || !app.snap || app.snap.nodes.length > 1 || app.view === 'nodes'))
  let title = $derived(VIEWS.find(([v]) => v === app.view)?.[1] ?? '')
  let connLine = $derived.by(() => {
    const c = app.conn
    if (!c) return 'connecting…'
    if (c.status === 'connecting') return 'connecting…'
    if (c.status === 'error') return 'not connected'
    return `${c.endpoint ?? ''}${c.latencyMs !== null ? ` · ${c.latencyMs} ms` : ''}`
  })
  let latest = $derived(app.update?.releases[0])
  let showUpdate = $derived(app.updateAvailable && !!latest && (app.dismissed !== latest.version || app.update?.phase !== 'available'))
  const THEME_ICON = { system: 'monitor', light: 'sun', dark: 'moon' } as const
  const THEME_NAME = { system: 'follow system', light: 'light', dark: 'dark' } as const
  const mac = /Mac|iPhone|iPad/.test(navigator.platform)

  // Two-key shortcuts: g then o / s / l / n / c / p.
  const GO: Record<string, View> = { o: 'overview', s: 'stacks', l: 'logs', n: 'nodes', c: 'connections', p: 'prefs' }
  let gAt = 0

  function keydown(e: KeyboardEvent) {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'k') {
      e.preventDefault()
      app.palette = !app.palette
      return
    }
    if (e.key === 'Escape') {
      if (app.palette) app.palette = false
      else if (app.drawer) app.drawer = null
      return
    }
    const tag = (document.activeElement as HTMLElement | null)?.tagName
    if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || app.palette || e.ctrlKey || e.metaKey || e.altKey) return
    if (gAt && Date.now() - gAt < 1000 && GO[e.key]) {
      gAt = 0
      app.drawer = null
      app.go(GO[e.key])
    } else {
      gAt = e.key === 'g' ? Date.now() : 0
    }
  }
</script>

<svelte:window onkeydown={keydown} />

<div class="app">
  <aside class="side">
    <div class="brand">
      <svg width="32" height="27" viewBox="0 0 96 80" aria-hidden="true">
        <use href="/brand/stackline-mark.svg#stackline" />
      </svg>
      <div>swarmdash<small>Read-only</small></div>
    </div>
    <nav class="nav">
      {#each views as [v, label] (v)}
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
      <ConnMenu />
      <span class="conn">{connLine}</span>
      <span class="spacer"></span>
      {#if showUpdate && latest}
        <button class="ctl update-pill" onclick={() => (app.drawer = { kind: 'update' })}>
          <Icon name="sparkle" size={14} />
          {#if app.update?.phase === 'downloading' || app.update?.phase === 'installing'}Updating…{:else if app.update?.phase === 'restarting'}Restarting…{:else}Update to {latest.version}{/if}
        </button>
      {/if}
      <button class="ctl cmdk" onclick={() => (app.palette = true)} aria-label="Jump to a service, stack or page">
        <Icon name="search" size={14} /><span class="t">Jump to a service, stack or page</span><kbd>{mac ? '⌘' : 'Ctrl'} K</kbd>
      </button>
      <button class="ctl icon" onclick={() => app.cycleTheme()} title="Theme: {THEME_NAME[app.theme]}. Click to change." aria-label="Theme: {THEME_NAME[app.theme]}">
        <Icon name={THEME_ICON[app.theme]} size={15} />
      </button>
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

{#if app.drawer?.kind === 'service'}
  {#key app.drawer.key}<ServiceDrawer key={app.drawer.key} tab={app.drawer.tab} />{/key}
{:else if app.drawer?.kind === 'node'}<NodeDrawer id={app.drawer.id} />
{:else if app.drawer?.kind === 'connection'}<ConnectionDrawer id={app.drawer.id} />
{:else if app.drawer?.kind === 'update'}<UpdateDrawer />{/if}

{#if app.palette}<Palette />{/if}

{#if app.toast}<div class="toast" class:err={app.toast.kind === 'err'} role="status">{app.toast.text}</div>{/if}
