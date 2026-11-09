<script lang="ts">
  // The shell, laid out like Kubri: the sidebar; a top bar with history,
  // connection, stack picker and command search; the page with the inspector
  // docked beside it; the log workspace below; and a status bar with the keys.
  import { onMount } from 'svelte'
  import { app, LIST_VIEWS, VIEWS, type View } from './lib/state.svelte'
  import Icon from './components/Icon.svelte'
  import Sidebar from './components/Sidebar.svelte'
  import StackPicker from './components/StackPicker.svelte'
  import Overview from './views/Overview.svelte'
  import Stacks from './views/Stacks.svelte'
  import Services from './views/Services.svelte'
  import Tasks from './views/Tasks.svelte'
  import Nodes from './views/Nodes.svelte'
  import Logs from './views/Logs.svelte'
  import Connections from './views/Connections.svelte'
  import Prefs from './views/Prefs.svelte'
  import ServiceDrawer from './components/ServiceDrawer.svelte'
  import NodeDrawer from './components/NodeDrawer.svelte'
  import ConnectionDrawer from './components/ConnectionDrawer.svelte'
  import UpdateDrawer from './components/UpdateDrawer.svelte'
  import ConnMenu from './components/ConnMenu.svelte'
  import Palette from './components/Palette.svelte'
  import HelpOverlay from './components/HelpOverlay.svelte'
  import Dock from './components/Dock.svelte'

  onMount(() => {
    app.init()
  })

  let title = $derived(VIEWS.find(([v]) => v === app.view)?.[1] ?? '')
  let latest = $derived(app.update?.releases[0])
  let showUpdate = $derived(app.updateAvailable && !!latest && (app.dismissed !== latest.version || app.update?.phase !== 'available'))
  let isList = $derived(LIST_VIEWS.includes(app.view))
  let needsConn = $derived(app.view !== 'connections' && app.view !== 'prefs')
  let docked = $derived(needsConn && !!app.snap && (app.drawer?.kind === 'service' || app.drawer?.kind === 'node'))
  const THEME_ICON = { system: 'monitor', light: 'sun', dark: 'moon' } as const
  const THEME_NAME = { system: 'follow system', light: 'light', dark: 'dark' } as const
  const mac = /Mac|iPhone|iPad/.test(navigator.platform)

  let status = $derived.by(() => {
    const c = app.conn
    if (!c || c.status === 'connecting') return { cls: 'off', text: `Connecting · ${app.active?.name ?? '…'}` }
    if (c.status === 'error') return { cls: 'err', text: `Not connected · ${app.active?.name ?? ''}` }
    return { cls: '', text: `Live · ${app.active?.name ?? ''}${c.latencyMs !== null ? ` · ${c.latencyMs} ms` : ''}` }
  })

  /** The keys the status bar names for the current page. */
  let hints = $derived.by((): [string, string][] => {
    const base: [string, string][] = [[':', 'command'], ['/', 'filter']]
    if (isList) {
      const open = app.view === 'stacks' ? 'services' : app.view === 'services' ? 'tasks' : app.view === 'nodes' ? 'tasks' : 'service'
      return [...base, ['↵', open], ['d', 'inspect'], ['l', 'logs'], ...(app.view === 'services' ? ([['p', 'pin']] as [string, string][]) : []), ['?', 'help']]
    }
    if (app.view === 'logs') return [[':', 'command'], ['/', 'search'], ['j k', 'move'], ['f', 'fill window'], ['?', 'help']]
    return [[':', 'command'], ['g s', 'services'], ['g l', 'logs'], ['?', 'help']]
  })

  // Two-key shortcuts: g then a letter.
  const GO: Record<string, View> = { o: 'overview', k: 'stacks', s: 'services', t: 'tasks', n: 'nodes', l: 'logs', c: 'connections', p: 'prefs' }

  function keydown(e: KeyboardEvent) {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'k') {
      e.preventDefault()
      if (app.palette) app.palette = false
      else app.openPalette()
      return
    }
    if (e.altKey && (e.key === 'ArrowLeft' || e.key === 'ArrowRight')) {
      e.preventDefault()
      if (e.key === 'ArrowLeft') history.back()
      else history.forward()
      return
    }
    if (e.key === 'Escape') {
      if (app.palette) app.palette = false
      else if (app.help) app.help = false
      else if (app.drawer) app.drawer = null
      else if (app.logs.maxi) app.logs.maxi = false
      return
    }
    const tag = (document.activeElement as HTMLElement | null)?.tagName
    if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || app.palette || app.help || e.ctrlKey || e.metaKey || e.altKey) return
    if (app.awaitingGo() && GO[e.key]) {
      app.goAt = 0
      app.drawer = null
      app.go(GO[e.key])
      e.preventDefault()
      // The list's own key for this letter must not run too.
      e.stopImmediatePropagation()
      return
    }
    app.goAt = e.key === 'g' ? Date.now() : 0
    if (e.key === ':') {
      e.preventDefault()
      app.openPalette()
    } else if (e.key === '?') {
      app.help = true
    } else if (e.key === 'w') {
      app.dock.toggle()
    }
  }
</script>

<svelte:window onkeydown={keydown} />

<div class="app">
  <Sidebar />
  <div class="main">
    <header class="top">
      <div class="history">
        <button class="sq" onclick={() => history.back()} title="Back (Alt ←)" aria-label="Back"><Icon name="back" size={16} /></button>
        <button class="sq" onclick={() => history.forward()} title="Forward (Alt →)" aria-label="Forward"><Icon name="forward" size={16} /></button>
      </div>
      <ConnMenu />
      {#if app.snap && app.snap.stacks.length}<span class="divider hide-sm"></span><StackPicker />{/if}
      <span class="spacer"></span>
      {#if showUpdate && latest}
        <button class="ctl update-pill" onclick={() => (app.drawer = { kind: 'update' })}>
          <Icon name="sparkle" size={14} />
          {#if app.update?.phase === 'downloading' || app.update?.phase === 'installing'}Updating…{:else if app.update?.phase === 'restarting'}Restarting…{:else}Update to {latest.version}{/if}
        </button>
      {/if}
      <button class="ctl cmdk" onclick={() => app.openPalette()} aria-label="Search or run a command">
        <Icon name="search" size={14} /><span class="t">Search or run a command</span><kbd>{mac ? '⌘' : 'Ctrl'} K</kbd>
      </button>
      <button class="sq bordered" onclick={() => app.cycleTheme()} title="Theme: {THEME_NAME[app.theme]}. Click to change." aria-label="Theme: {THEME_NAME[app.theme]}">
        <Icon name={THEME_ICON[app.theme]} size={15} />
      </button>
      <span class="ro-badge" title="Only GET calls reach the Docker API. Nothing in Stackline can change your containers."><Icon name="lock" size={13} /><span class="hide-sm">Read-only</span></span>
      <button class="sq bordered help-btn" onclick={() => (app.help = true)} title="Keys and commands (?)" aria-label="Keys and commands">?</button>
    </header>
    <div class="area">
      <div class="content" class:with-inspector={docked} class:full={docked && app.inspectorFull}>
        <main class="view" class:list={isList}>
          {#if app.fatal}
            <div class="center-empty"><b style="color:var(--fg)">{app.fatal}</b><button class="btn" onclick={() => location.reload()}>Try again</button></div>
          {:else if app.view === 'connections'}
            <div class="phead"><nav class="crumbs" aria-label="Where you are"><span>Stackline</span><Icon name="chev" size={12} /><span>{title}</span></nav><div class="ptitle"><h2>{title}</h2></div></div>
            <Connections />
          {:else if app.view === 'prefs'}
            <div class="phead"><nav class="crumbs" aria-label="Where you are"><span>Stackline</span><Icon name="chev" size={12} /><span>{title}</span></nav><div class="ptitle"><h2>{title}</h2></div></div>
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
            {#if !isList}
              <div class="phead">
                <nav class="crumbs" aria-label="Where you are"><span>{app.active?.name}</span><Icon name="chev" size={12} /><span>{title}</span></nav>
                <div class="ptitle"><h2>{title}</h2></div>
              </div>
            {/if}
            <!-- Views start fresh for each connection, so nothing from another host lingers. -->
            {#key app.activeId}
              {#if app.view === 'overview'}<Overview />
              {:else if app.view === 'stacks'}<Stacks />
              {:else if app.view === 'services'}<Services />
              {:else if app.view === 'tasks'}<Tasks />
              {:else if app.view === 'nodes'}<Nodes />
              {:else if app.view === 'logs'}<Logs />{/if}
            {/key}
          {/if}
        </main>
        {#if docked && app.drawer?.kind === 'service'}
          {#key app.drawer.key}<ServiceDrawer key={app.drawer.key} tab={app.drawer.tab} />{/key}
        {:else if docked && app.drawer?.kind === 'node'}<NodeDrawer id={app.drawer.id} />{/if}
      </div>
      {#if app.dock.tabs.length && app.snap}<Dock />{/if}
    </div>
    <footer class="statusbar">
      <span class="status {status.cls}"><span class="dot {status.cls}"></span>{status.text}</span>
      <span class="spacer"></span>
      <span class="hints hide-sm">
        {#each hints as [k, what] (k)}<span><kbd>{k}</kbd>{what}</span>{/each}
      </span>
    </footer>
  </div>
</div>

{#if app.drawer?.kind === 'connection'}<ConnectionDrawer id={app.drawer.id} />
{:else if app.drawer?.kind === 'update'}<UpdateDrawer />{/if}

{#if app.palette}<Palette />{/if}
{#if app.help}<HelpOverlay />{/if}

{#if app.toast}<div class="toast" class:err={app.toast.kind === 'err'} role="status">{app.toast.text}</div>{/if}
