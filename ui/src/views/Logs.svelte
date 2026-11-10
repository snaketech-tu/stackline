<script lang="ts">
  import { onDestroy, untrack } from 'svelte'
  import { app, LEVELS } from '../lib/state.svelte'
  import { api, fetchEvents, fetchLogs } from '../lib/api'
  import { LINES, LogPages, nearEnd } from '../lib/pages.svelte'
  import { ago, bytes, dateTime, firstLine, hhmm, hms, patternParts, plural, searchTerms } from '../lib/format'
  import type { DockerEvent, Entry, Group, Level, StoreStats } from '../lib/types'
  import { isCrash } from '../lib/events'
  import { pointText } from '../lib/timeframe'
  import Histogram from '../components/Histogram.svelte'
  import Icon from '../components/Icon.svelte'
  import LogRow from '../components/LogRow.svelte'
  import SourcePicker from '../components/SourcePicker.svelte'
  import RangePicker from '../components/RangePicker.svelte'
  import DownloadMenu from '../components/DownloadMenu.svelte'
  import Spark from '../components/Spark.svelte'

  const L = app.logs
  const COL: Record<Level, string> = { ERROR: 'var(--err)', WARN: 'var(--warn)', INFO: 'var(--info)', DEBUG: 'var(--dbg)' }

  // The newest page is replaced as a whole on every load; older pages are added by "Show more".
  const pages = new LogPages()
  let res = $derived(pages.head)
  let events = $state.raw<DockerEvent[]>([])
  let shown = $state({ from: 0, to: 0 })
  let loading = $state(false)
  let error = $state<string | null>(null)
  let stats = $state<StoreStats | null>(null)
  let qInput = $state(L.q)
  let search: HTMLInputElement | undefined = $state()
  let streamEl: HTMLElement | undefined = $state()
  let seq = 0
  /** Open the newest entry once the next result arrives (after picking a group). */
  let openFirst = false
  let rangeOpen = $state(false)

  async function load() {
    const id = app.activeId
    const p = L.params()
    const mine = ++seq
    loading = true
    try {
      // Events are a side note: the lines show even when they cannot be read.
      const hp = pages.headParams({ ...p, buckets: 60 })
      const [r, ev] = await Promise.all([fetchLogs(id, hp), fetchEvents(id, p).catch(() => [])])
      if (mine !== seq || !pages.takeHead(r, hp)) return
      events = ev
      shown = { from: p.from, to: p.to }
      error = null
      if (openFirst && r.result.entries[0]) L.openRows.add(r.result.entries[0].id)
      openFirst = false
    } catch (e) {
      if (mine === seq) error = app.describe(e)
    } finally {
      if (mine === seq) loading = false
    }
  }

  // Reload when a filter changes (debounced), and keep a preset range moving.
  $effect(() => {
    void [app.activeId, [...L.sources].join(), L.q, L.group, L.preset, L.openEnd, L.from, L.to, [...L.levels].join()]
    // Other filters: the older lines shown so far no longer fit.
    untrack(() => pages.reset())
    const t = setTimeout(load, 80)
    return () => clearTimeout(t)
  })
  $effect(() => {
    if (!L.moving) return
    const every = L.live ? Math.max(1, app.settings?.logPollSecs ?? 3) * 1000 : 15000
    // A hidden tab catches up when it is shown again instead of polling in the background.
    const t = setInterval(() => !document.hidden && load(), every)
    return () => clearInterval(t)
  })
  $effect(() => {
    api<StoreStats>('/store').then((s) => (stats = s)).catch(() => {})
  })

  // Search box: typed text is applied after a short pause.
  let typed: string | null = null
  $effect(() => {
    const v = (typed = qInput)
    const t = setTimeout(() => {
      L.q = v
      typed = null
    }, 250)
    return () => clearTimeout(t)
  })
  // Leaving the page within that pause keeps what was typed, unless the
  // page closes because another connection was picked.
  const conn = app.activeId
  onDestroy(() => {
    if (typed !== null && app.activeId === conn) L.q = typed
  })
  $effect(() => {
    // Filters set from elsewhere (a chip, the overview) show up in the box.
    qInput = L.q
  })

  function toggleLive() {
    L.live = !L.live
    if (L.live && !L.moving) L.setPreset('auto')
  }

  function setQ(q: string) {
    L.q = q
    qInput = q
  }

  let terms = $derived(searchTerms(L.q))
  let crashes = $derived(events.filter(isCrash).length)

  type Row = { e: Entry; n: number }
  /** The rows of the last result, to hand unchanged rows back as the same objects. */
  let shownRows = new Map<number, Row>()
  let rows = $derived.by(() => {
    const out: Row[] = []
    let last: Row | null = null
    for (const e of pages.entries) {
      if (L.collapse && last && e.fp && last.e.fp === e.fp) {
        last.n++
        continue
      }
      last = { e, n: 1 }
      out.push(last)
    }
    const next = new Map<number, Row>()
    const stable = out.map((r) => {
      const old = shownRows.get(r.e.id)
      const keep = old && old.e === r.e && old.n === r.n ? old : r
      next.set(r.e.id, keep)
      return keep
    })
    shownRows = next
    return stable
  })

  let groups = $derived(res?.result.groups ?? [])
  let groupEvents = $derived(groups.reduce((a, g) => a + g.count, 0))
  let fresh = $derived(new Set(app.errors?.newGroups.map((g) => g.fp) ?? []))
  // The picked group stays listed while the stream is narrowed to it.
  let picked = $state<Group | null>(null)

  function pickGroup(g: Group) {
    L.openRows.clear()
    if (L.group === g.fp) {
      L.group = null
      picked = null
      return
    }
    L.group = g.fp
    picked = g
    openFirst = true
    if (innerWidth < 1100 || !side) streamEl?.scrollIntoView({ behavior: 'smooth', block: 'start' })
  }
  let groupLabel = $derived.by(() => {
    if (!L.group) return ''
    const g = groups.find((x) => x.fp === L.group) ?? picked
    return g ? g.pattern || firstLine(g.sample) : L.group
  })

  function onlyService(svc: string) {
    L.sources.clear()
    L.sources.add(svc)
  }
  /**
   * The 30 seconds around a line, from the same container: what it printed
   * before and after. Removing the container chip widens it to the host.
   */
  function context(e: Entry) {
    L.zoom(e.ts / 1000 - 30000, e.ts / 1000 + 30000)
    L.sources.clear()
    L.sources.add(e.service)
    L.group = null
    L.allLevels()
    setQ(`container:${e.source.slice(0, 12)}`)
  }
  function clearAll() {
    L.sources.clear()
    L.allLevels()
    L.group = null
    picked = null
    setQ('')
  }

  function keydown(e: KeyboardEvent) {
    const tag = (document.activeElement as HTMLElement | null)?.tagName
    if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || app.drawer || app.palette || app.help || app.ask || e.ctrlKey || e.metaKey || e.altKey) return
    if (e.key === 'f') {
      L.maxi = !L.maxi
    } else if (e.key === 't') {
      e.preventDefault()
      rangeOpen = true
    } else if (e.key === '[' || e.key === ']') {
      L.step(e.key === '[' ? -1 : 1)
    } else if (e.key === '-') {
      L.zoomOut()
    } else if (e.key === '/') {
      e.preventDefault()
      search?.focus()
    } else if (e.key === 'j' || e.key === 'k') {
      // Move through the stream rows.
      const list = [...(streamEl?.querySelectorAll<HTMLElement>('.stream > .row') ?? [])]
      if (!list.length) return
      const i = list.indexOf(document.activeElement as HTMLElement)
      const next = list[Math.max(0, Math.min(list.length - 1, i < 0 ? 0 : i + (e.key === 'j' ? 1 : -1)))]
      next.focus()
      next.scrollIntoView({ block: 'nearest' })
    }
  }

  let side = $derived(L.groupsAt === 'side' && L.groupsOpen && !L.maxi)

  const sourceLabel = (s: string) => (s.startsWith('stack:') ? `stack ${s.slice(6)}` : s)

  let last = $derived(pages.entries.at(-1))
</script>

<svelte:window onkeydown={keydown} />

<section class="panel">
  <div class="pb fbar">
    <RangePicker bind:open={rangeOpen} />
    <SourcePicker />
    <span class="lvgroup" role="group" aria-label="Levels">
      {#each LEVELS as l (l)}
        <button class="lv" aria-pressed={L.levels.has(l)} onclick={(e) => L.pickLevel(l, e.shiftKey || e.metaKey || e.ctrlKey)}
          title="Click: only {l.toLowerCase()}. Shift-click: add or remove {l.toLowerCase()}.">
          <span class="sw" style:background={COL[l]}></span>{l[0] + l.slice(1).toLowerCase()}<span class="n">{res?.result.levels[l] ?? 0}</span>
        </button>
      {/each}
    </span>
  </div>
  <div class="pb fbar" style="padding-top:0">
    <label class="search">
      <Icon name="search" size={14} />
      <input bind:this={search} bind:value={qInput} placeholder="Search, or service:api level:error replica:2" autocomplete="off" spellcheck="false" />
      {#if qInput}<button class="ib" style="width:22px;height:22px" onclick={() => setQ('')} aria-label="Clear search"><Icon name="x" size={12} /></button>{:else}<kbd class="hide-sm">/</kbd>{/if}
    </label>
    <button class="ctl" class:live={L.live} aria-pressed={L.live} onclick={toggleLive}><span class="live-dot"></span>Live</button>
    <button class="ctl" onclick={() => app.copy(app.logLink(), 'Link copied. It opens this view with the same time range, in a browser or pasted into Ctrl K.')}
      title="Copy a link to this view, with its time range fixed"><Icon name="link" size={14} /><span class="hide-sm">Copy link</span></button>
    <DownloadMenu />
  </div>
  <div class="ph" style="border-top:1px solid var(--line);border-bottom:0;padding-bottom:0">
    <h2>Log volume</h2>
    <span class="muted mono" style="font-size:12px">
      {#if L.preset === 'auto'}since {hhmm(shown.from || L.window()[0])}{:else}{L.rangeLabel(app.now)}{/if}
    </span>
    {#if L.preset !== 'auto'}<button class="btn ghost sm" onclick={() => L.zoomOut()} title="Twice as long ( - )">Zoom out</button>{/if}
    {#if !L.preset}<button class="btn ghost sm" onclick={() => L.setPreset('auto')} title="Back to all stored lines">Reset range</button>{/if}
    {#if loading}<span class="spinner" style="width:12px;height:12px"></span>{/if}
    <span class="pill ok hide-sm" title="Logs are collected per replica and kept on this computer" style="font-size:11.5px">
      <span class="dot"></span>Local store · {app.settings?.retentionDays ?? 2} {app.settings?.retentionDays === 1 ? 'day' : 'days'}{#if stats} · {bytes(stats.bytes)}{/if}{#if res?.syncedAt} · synced {ago(res.syncedAt, app.now)}{/if}
    </span>
    <span class="spacer"></span>
    <div class="legend">
      <span><i style="background:var(--err)"></i>Error</span><span><i style="background:var(--warn)"></i>Warn</span>
      <span><i style="background:var(--info);opacity:.6"></i>Info</span><span><i style="background:var(--dbg);opacity:.6"></i>Debug</span>
      <span><i class="line" style="background:var(--exc)"></i>Exceptions</span>
      {#if events.length}
        <span title="Containers that started, stopped, crashed or failed a health check in this range"><i class="mark" style="background:var(--err)"></i>{crashes ? plural(crashes, 'crash', 'crashes') : 'Starts and stops'}</span>
      {/if}
    </div>
  </div>
  {#if res}
    <Histogram buckets={res.result.histogram} from={shown.from} to={shown.to} marks={events} onrange={(a, z) => L.zoom(a, z)} />
  {:else}
    <div class="center-empty" style="padding:70px"><span class="spinner"></span></div>
  {/if}
</section>

{#if error}<div class="err-banner">{error}</div>{/if}
{#if res?.logErrors.length}
  <div class="warn-banner"><span><b>Some logs could not be read.</b><br /><span class="muted" style="font-size:12.5px">{res.logErrors.slice(0, 3).join(' · ')}</span></span></div>
{/if}

<div class="logs-grid" class:side>
  {#if L.groupsOpen && !L.maxi}
  <section class="panel groups" class:above={!side}>
    <div class="ph">
      <h2>Error groups</h2>
      {#if groups.length}<span class="muted" style="font-size:12px">{plural(groups.length, 'group')} · {groupEvents.toLocaleString()} events</span>{/if}
      <span class="spacer"></span>
      <button class="ib hide-sm" onclick={() => L.setGroupsAt(side ? 'top' : 'side')}
        title={side ? 'Put the groups above the stream, so the lines get the full width' : 'Put the groups beside the stream'}
        aria-label={side ? 'Groups above the stream' : 'Groups beside the stream'}><Icon name={side ? 'stacked' : 'sideBySide'} size={15} /></button>
      <button class="ib" onclick={() => L.toggleGroups()} title="Hide the error groups" aria-label="Hide the error groups"><Icon name="x" size={15} /></button>
    </div>
    <div class="glist">
    {#each groups as g (g.fp)}
      <button class="grp" class:sel={L.group === g.fp} aria-pressed={L.group === g.fp} onclick={() => pickGroup(g)}
        title={L.group === g.fp ? 'Show all lines again' : 'Show only this group in the stream'}>
        <span class="count" class:w={g.level === 'WARN'}>{g.count}</span>
        <span class="ttl" title={g.pattern || firstLine(g.sample)}>{#each patternParts(g.pattern || firstLine(g.sample)) as part, i (i)}{#if part.v}<var>{part.text}</var>{:else}{part.text}{/if}{/each}</span>
        <Spark values={g.spark} warn={g.level === 'WARN'} />
        <span class="meta">
          <span class="sv">{g.services.join(', ')}</span>
          {#if g.exc}<span style="color:var(--exc)">exception</span>{:else}<span>{g.level === 'WARN' ? 'warning' : 'log error'}</span>{/if}
          <span>· last {ago(g.last / 1000, app.now)}</span>
          {#if app.newSinceDeploy.get(g.fp)}{@const d = app.newSinceDeploy.get(g.fp)!}<span class="new" title="First seen after {d.event.service} was deployed at {hhmm(d.event.ts / 1000)}">new since deploy</span>
          {:else if fresh.has(g.fp)}<span class="new">new</span>{/if}
        </span>
      </button>
    {:else}
      <div class="empty">{res ? 'No errors match these filters.' : ''}</div>
    {/each}
    </div>
  </section>
  {/if}

  {#if L.maxi}<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions --><div class="scrim" onclick={() => (L.maxi = false)}></div>{/if}
  <section class="panel streampanel" class:maxi={L.maxi} bind:this={streamEl}>
    <div class="ph">
      <h2>Log stream</h2>
      {#if res}<span class="muted" style="font-size:12px">{res.result.total.toLocaleString()} lines{#if pages.more} · newest {pages.entries.length.toLocaleString()} shown{/if}</span>{/if}
      {#if L.maxi}<span class="pill hide-sm">{app.active?.name}</span>{/if}
      <span class="spacer"></span>
      {#if !L.groupsOpen && !L.maxi}
        <button class="btn ghost sm" onclick={() => L.toggleGroups()}>Error groups{#if groups.length}<span class="pill err" style="margin-left:4px">{groups.length}</span>{/if}</button>
      {/if}
      <label class="toggle"><span class="switch"><input type="checkbox" bind:checked={L.collapse} /><i></i></span><span class="hide-sm">Collapse repeats</span></label>
      <button class="ib" onclick={() => (L.maxi = !L.maxi)} title={L.maxi ? 'Back to the page (Esc or f)' : 'Fill the window with the stream (f)'}
        aria-label={L.maxi ? 'Back to the page' : 'Fill the window with the stream'}><Icon name={L.maxi ? 'minimize' : 'maximize'} size={15} /></button>
    </div>
    {#if L.sources.size || L.soloLevels || L.group || L.q}
      <div class="chiprow">
        <span class="muted">Filters</span>
        {#each [...L.sources] as s (s)}
          <button class="fchip" onclick={() => L.sources.delete(s)} aria-label="Remove {sourceLabel(s)}"><span class="t">{sourceLabel(s)}</span><span class="x"><Icon name="x" size={11} /></span></button>
        {/each}
        {#if L.soloLevels}
          <button class="fchip" onclick={() => L.allLevels()} aria-label="Show all levels"><span class="t">{[...L.levels].map((l) => l.toLowerCase()).join(' + ')}</span><span class="x"><Icon name="x" size={11} /></span></button>
        {/if}
        {#if L.group}
          <button class="fchip g" onclick={() => { L.group = null; picked = null }} aria-label="Show all groups"><span class="t">group: {groupLabel}</span><span class="x"><Icon name="x" size={11} /></span></button>
        {/if}
        {#if L.q}
          <button class="fchip" onclick={() => setQ('')} aria-label="Clear search"><span class="t">“{L.q}”</span><span class="x"><Icon name="x" size={11} /></span></button>
        {/if}
        <button class="btn ghost sm" onclick={clearAll}>Clear all</button>
      </div>
    {/if}
    <div class="stream">
      {#each rows as { e, n } (e.id)}
        <LogRow {e} {n} {terms} open={L.openRows.has(e.id)} ontoggle={() => L.toggleRow(e.id)}
          onservice={() => onlyService(e.service)} oncontext={() => context(e)} />
      {:else}
        <div class="empty">{res ? 'No log lines match. Remove a filter or pick a wider range.' : ''}</div>
      {/each}
      {#if res && rows.length}
        <!-- Where the lines end, said in place: more to load, the most shown, or the start of what is stored. -->
        <div class="stream-end" class:busy={pages.loadingOlder} use:nearEnd={() => pages.loadOlder(app.activeId, L.params(), app.describe)}>
          {#if pages.olderError}
            <span class="err-text">Older lines could not be read: {pages.olderError}</span>
            <button class="btn ghost sm" onclick={() => pages.retry(app.activeId, L.params(), app.describe)}>Try again</button>
          {:else if pages.more && !pages.full}
            <button class="btn sm" onclick={() => pages.loadOlder(app.activeId, L.params(), app.describe)} disabled={pages.loadingOlder}>
              {#if pages.loadingOlder}<span class="spinner" style="width:12px;height:12px"></span>Loading older lines{:else}Show {LINES.step.toLocaleString()} older lines{/if}
            </button>
            <span class="muted">{pages.entries.length.toLocaleString()} of {res.result.total.toLocaleString()} shown</span>
          {:else if pages.more}
            <span>The stream shows at most {LINES.max.toLocaleString()} lines. For older ones, pick an earlier range (<kbd>[</kbd> or type one under <kbd>t</kbd>), or download them.</span>
            <button class="btn ghost sm" onclick={() => L.step(-1)}>Earlier range</button>
          {:else if L.preset === 'auto'}
            <span class="muted">These are all the stored lines that match{#if last}, back to {app.now - last.ts / 1000 > 20 * 3600e3 ? dateTime(last.ts / 1000) : hms(last.ts / 1000).slice(0, 8)}{/if}. Stackline keeps {app.settings?.retentionDays ?? 2} {app.settings?.retentionDays === 1 ? 'day' : 'days'} of logs, and reads a replica 6 hours back when it first sees it.</span>
          {:else}
            <span class="muted">No older lines in this range, which starts at {pointText(shown.from, app.now)}.</span>
            <button class="btn ghost sm" onclick={() => L.step(-1)}>Earlier range</button>
            <button class="btn ghost sm" onclick={() => L.setPreset('auto')}>Show all stored lines</button>
          {/if}
        </div>
      {/if}
    </div>
    <div class="stream-foot">
      {#if L.live}<span class="live-dot" style="background:var(--err)"></span>Following new lines every {app.settings?.logPollSecs ?? 3} s.{:else}Updates every 15 s. Turn on Live to follow new lines.{/if}
      <span class="spacer"></span><span class="hide-sm">j / k move · Enter expands · c copies · f fills the window</span>
    </div>
  </section>
</div>
