<script lang="ts">
  // The sidebar: every page by group, the pinned services, and a box that
  // finds a page or service by name.
  import { app, VIEWS, type View } from '../lib/state.svelte'
  import { svcProblem, svcState } from '../lib/health'
  import Icon from './Icon.svelte'

  let find = $state('')
  let needle = $derived(find.trim().toLowerCase())
  let latest = $derived(app.update?.releases[0])

  const ICONS: Partial<Record<View, string>> = { services: 'services', tasks: 'tasks' }
  let counts = $derived<Partial<Record<View, { n: number; bad?: boolean; title: string }>>>({
    stacks: { n: app.snap?.stacks.length ?? 0, title: 'Stacks' },
    services: app.services.some(svcProblem)
      ? { n: app.services.filter(svcProblem).length, bad: true, title: 'Services that need attention' }
      : { n: app.services.length, title: 'Services' },
    tasks: { n: app.services.reduce((a, s) => a + s.tasks.length, 0), title: 'Tasks' },
    nodes: { n: app.snap?.nodes.length ?? 0, title: 'Nodes' },
    logs: app.errors?.lastHour ? { n: app.errors.lastHour, bad: true, title: 'Errors in the last hour' } : undefined,
  })

  let groups = $derived.by(() => {
    const out: [string, [View, string][]][] = []
    for (const [v, label, group] of VIEWS) {
      if (group === 'App') continue
      if (needle && !label.toLowerCase().includes(needle)) continue
      const g = out.find(([x]) => x === group)
      if (g) g[1].push([v, label])
      else out.push([group, [[v, label]]])
    }
    return out
  })
  let pinned = $derived(app.pinnedServices.filter((s) => !needle || s.fullName.toLowerCase().includes(needle)))
  let found = $derived(
    needle ? app.services.filter((s) => !app.isPinned(s.key) && s.fullName.toLowerCase().includes(needle)).slice(0, 8) : [],
  )

  function openSvc(key: string) {
    app.lists.services.cursor = key
    if (app.view !== 'services' && app.view !== 'overview') app.drill('services')
    app.openService(key)
  }

  function findKey(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      find = ''
      ;(e.currentTarget as HTMLInputElement).blur()
    } else if (e.key === 'Enter') {
      const v = groups[0]?.[1][0]
      const s = pinned[0] ?? found[0]
      if (v) app.go(v[0])
      else if (s) openSvc(s.key)
      else app.openPalette(find)
      find = ''
      ;(e.currentTarget as HTMLInputElement).blur()
    }
  }
</script>

<aside class="side">
  <div class="brand">
    <svg width="30" height="25" viewBox="0 0 96 80" aria-hidden="true"><use href="/brand/stackline-mark.svg#stackline" /></svg>
    <span class="bname">Stackline</span>
    {#if app.meta}<span class="ver mono">v{app.meta.version}</span>{/if}
  </div>
  <label class="find">
    <input bind:value={find} onkeydown={findKey} placeholder="Find a page or service…" autocomplete="off" spellcheck="false" aria-label="Find a page or service" />
  </label>
  <nav class="nav" aria-label="Pages">
    {#each groups as [group, views] (group)}
      {#if group}<div class="grp-h">{group}</div>{/if}
      {#each views as [v, label] (v)}
        {@const c = counts[v]}
        <a href="#{v}" aria-current={app.view === v ? 'page' : undefined} onclick={() => (find = '')}>
          <Icon name={ICONS[v] ?? v} /><span class="t">{v === 'tasks' && app.snap?.mode === 'standalone' ? 'Containers' : label}</span>
          {#if c && c.n}<span class="cnt" class:bad={c.bad} title={c.title}>{c.n}</span>{/if}
        </a>
      {/each}
    {/each}
    {#if pinned.length}
      <div class="grp-h">Pinned</div>
      {#each pinned as s (s.key)}
        {@const [cls] = svcState(s)}
        <button class="svc-link" class:on={app.drawer?.kind === 'service' && app.drawer.key === s.key} onclick={() => openSvc(s.key)} title={s.fullName}>
          <span class="dot {cls === 'ok' ? '' : cls || 'off'}"></span><span class="t">{s.name}</span><span class="st">{s.stack}</span>
        </button>
      {/each}
    {/if}
    {#if found.length}
      <div class="grp-h">Services</div>
      {#each found as s (s.key)}
        {@const [cls] = svcState(s)}
        <button class="svc-link" onclick={() => { openSvc(s.key); find = '' }} title={s.fullName}>
          <span class="dot {cls === 'ok' ? '' : cls || 'off'}"></span><span class="t">{s.name}</span><span class="st">{s.stack}</span>
        </button>
      {/each}
    {/if}
  </nav>
  <div class="side-foot">
    <a href="#connections" aria-current={app.view === 'connections' ? 'page' : undefined}><Icon name="connections" /><span class="t">Connections</span></a>
    <a href="#prefs" aria-current={app.view === 'prefs' ? 'page' : undefined}><Icon name="prefs" /><span class="t">Preferences</span></a>
    <button onclick={() => (app.help = true)}><Icon name="keyboard" /><span class="t">Shortcuts &amp; help</span><kbd>?</kbd></button>
    {#if app.updateAvailable && latest}
      <button class="upd" onclick={() => (app.drawer = { kind: 'update' })}><Icon name="sparkle" /><span class="t">v{latest.version} available</span></button>
    {/if}
  </div>
</aside>
