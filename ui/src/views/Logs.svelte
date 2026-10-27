<script lang="ts">
  import { app, LEVELS, PRESETS, type Preset } from '../lib/state.svelte'
  import { api, query } from '../lib/api'
  import { ago, bytes, firstLine, hhmm, patternParts, plural, searchTerms } from '../lib/format'
  import type { Entry, Group, Level, LogResponse, StoreStats } from '../lib/types'
  import Histogram from '../components/Histogram.svelte'
  import Icon from '../components/Icon.svelte'
  import LogRow from '../components/LogRow.svelte'
  import SourcePicker from '../components/SourcePicker.svelte'
  import DownloadMenu from '../components/DownloadMenu.svelte'
  import Spark from '../components/Spark.svelte'

  const L = app.logs
  const COL: Record<Level, string> = { ERROR: 'var(--err)', WARN: 'var(--warn)', INFO: 'var(--info)', DEBUG: 'var(--dbg)' }
  const RANGES: Preset[] = ['auto', ...(Object.keys(PRESETS) as Preset[])]

  let res = $state<LogResponse | null>(null)
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

  async function load() {
    const id = app.activeId
    const p = L.params()
    const mine = ++seq
    loading = true
    try {
      const r = await api<LogResponse>(`/c/${encodeURIComponent(id)}/logs?${query({ ...p, buckets: 60, limit: 300 })}`)
      if (mine !== seq) return
      res = r
      shown = { from: +p.from, to: +p.to }
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
    void [app.activeId, [...L.sources].join(), L.q, L.group, L.preset, L.from, L.to, [...L.levels].join()]
    const t = setTimeout(load, 80)
    return () => clearTimeout(t)
  })
  $effect(() => {
    if (!L.preset) return
    const every = L.live ? Math.max(1, app.settings?.logPollSecs ?? 3) * 1000 : 15000
    const t = setInterval(load, every)
    return () => clearInterval(t)
  })
  $effect(() => {
    api<StoreStats>('/store').then((s) => (stats = s)).catch(() => {})
  })

  // Search box: typed text is applied after a short pause.
  $effect(() => {
    const v = qInput
    const t = setTimeout(() => (L.q = v), 250)
    return () => clearTimeout(t)
  })
  $effect(() => {
    // Filters set from elsewhere (a chip, the overview) show up in the box.
    qInput = L.q
  })

  function toggleLive() {
    L.live = !L.live
    if (L.live && !L.preset) L.setPreset('auto')
  }

  function setQ(q: string) {
    L.q = q
    qInput = q
  }

  let terms = $derived(searchTerms(L.q))

  type Row = { e: Entry; n: number }
  let rows = $derived.by(() => {
    const out: Row[] = []
    let last: Row | null = null
    for (const e of res?.result.entries ?? []) {
      if (L.collapse && last && e.fp && last.e.fp === e.fp) {
        last.n++
        continue
      }
      last = { e, n: 1 }
      out.push(last)
    }
    return out
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
    if (innerWidth < 1100) streamEl?.scrollIntoView({ behavior: 'smooth', block: 'start' })
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
  function context(e: Entry) {
    L.zoom(e.ts / 1000 - 30000, e.ts / 1000 + 30000)
    L.sources.clear()
    L.group = null
    setQ('')
  }
  function clearAll() {
    L.sources.clear()
    L.levels.clear()
    LEVELS.forEach((l) => L.levels.add(l))
    L.group = null
    picked = null
    setQ('')
  }

  function keydown(e: KeyboardEvent) {
    const tag = (document.activeElement as HTMLElement | null)?.tagName
    if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || app.drawer || app.palette || e.ctrlKey || e.metaKey || e.altKey) return
    if (e.key === '/') {
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

  const sourceLabel = (s: string) => (s.startsWith('stack:') ? `stack ${s.slice(6)}` : s)
  let soloLevels = $derived(L.levels.size < LEVELS.length)
</script>

<svelte:window onkeydown={keydown} />

<section class="panel">
  <div class="pb fbar">
    <div class="seg" role="group" aria-label="Time range">
      {#each RANGES as p (p)}
        <button aria-pressed={L.preset === p} onclick={() => L.setPreset(p)} title={p === 'auto' ? 'From the oldest stored line until now' : undefined}>{p === 'auto' ? 'Auto' : p}</button>
      {/each}
    </div>
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
    <DownloadMenu />
  </div>
  <div class="ph" style="border-top:1px solid var(--line);border-bottom:0;padding-bottom:0">
    <h2>Log volume</h2>
    <span class="muted mono" style="font-size:12px">
      {#if L.preset === 'auto'}since {hhmm(shown.from || L.window()[0])}{:else if L.preset}last {L.preset}{:else}{hhmm(L.from)} – {hhmm(L.to)}
        <button class="btn ghost sm" onclick={() => L.setPreset('auto')}>Reset zoom</button>{/if}
    </span>
    {#if loading}<span class="spinner" style="width:12px;height:12px"></span>{/if}
    <span class="pill ok hide-sm" title="Logs are collected per replica and kept on this computer" style="font-size:11.5px">
      <span class="dot"></span>Local store · {app.settings?.retentionDays ?? 7} days{#if stats} · {bytes(stats.bytes)}{/if}{#if res?.syncedAt} · synced {ago(res.syncedAt, app.now)}{/if}
    </span>
    <span class="spacer"></span>
    <div class="legend">
      <span><i style="background:var(--err)"></i>Error</span><span><i style="background:var(--warn)"></i>Warn</span>
      <span><i style="background:var(--info);opacity:.6"></i>Info</span><span><i style="background:var(--dbg);opacity:.6"></i>Debug</span>
      <span><i class="line" style="background:var(--exc)"></i>Exceptions</span>
    </div>
  </div>
  {#if res}
    <Histogram buckets={res.result.histogram} from={shown.from} to={shown.to} onrange={(a, z) => L.zoom(a, z)} />
  {:else}
    <div class="center-empty" style="padding:70px"><span class="spinner"></span></div>
  {/if}
</section>

{#if error}<div class="err-banner">{error}</div>{/if}
{#if res?.logErrors.length}
  <div class="warn-banner"><span><b>Some logs could not be read.</b><br /><span class="muted" style="font-size:12.5px">{res.logErrors.slice(0, 3).join(' · ')}</span></span></div>
{/if}

<div class="logs-grid">
  <section class="panel">
    <div class="ph">
      <h2>Error groups</h2>
      {#if groups.length}<span class="muted" style="font-size:12px">{plural(groups.length, 'group')} · {groupEvents.toLocaleString()} events</span>{/if}
    </div>
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
          {#if fresh.has(g.fp)}<span class="new">new</span>{/if}
        </span>
      </button>
    {:else}
      <div class="empty">{res ? 'No errors match these filters.' : ''}</div>
    {/each}
  </section>

  <section class="panel" bind:this={streamEl}>
    <div class="ph">
      <h2>Log stream</h2>
      {#if res}<span class="muted" style="font-size:12px">{res.result.total.toLocaleString()} lines{#if res.result.total > res.result.entries.length} · newest {res.result.entries.length}{/if}</span>{/if}
      <span class="spacer"></span>
      <label class="toggle"><span class="switch"><input type="checkbox" bind:checked={L.collapse} /><i></i></span>Collapse repeats</label>
    </div>
    {#if L.sources.size || soloLevels || L.group || L.q}
      <div class="chiprow">
        <span class="muted">Filters</span>
        {#each [...L.sources] as s (s)}
          <button class="fchip" onclick={() => L.sources.delete(s)} aria-label="Remove {sourceLabel(s)}"><span class="t">{sourceLabel(s)}</span><span class="x"><Icon name="x" size={11} /></span></button>
        {/each}
        {#if soloLevels}
          <button class="fchip" onclick={() => LEVELS.forEach((l) => L.levels.add(l))} aria-label="Show all levels"><span class="t">{[...L.levels].map((l) => l.toLowerCase()).join(' + ')}</span><span class="x"><Icon name="x" size={11} /></span></button>
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
        <LogRow {e} {n} {terms} open={L.openRows.has(e.id)}
          ontoggle={() => (L.openRows.has(e.id) ? L.openRows.delete(e.id) : L.openRows.add(e.id))}
          onservice={() => onlyService(e.service)} oncontext={() => context(e)} />
      {:else}
        <div class="empty">{res ? 'No log lines match. Remove a filter or pick a wider range.' : ''}</div>
      {/each}
    </div>
    <div class="stream-foot">
      {#if L.live}<span class="live-dot" style="background:var(--err)"></span>Following new lines every {app.settings?.logPollSecs ?? 3} s.{:else}Updates every 15 s. Turn on Live to follow new lines.{/if}
      <span class="spacer"></span><span class="hide-sm">j / k move · Enter expands · c copies</span>
    </div>
  </section>
</div>
