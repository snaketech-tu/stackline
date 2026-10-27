<script lang="ts">
  import { app, PRESETS, type Preset } from '../lib/state.svelte'
  import { api, query } from '../lib/api'
  import { ago, bytes, firstLine, hhmm, hms, patternParts, plural, tidy } from '../lib/format'
  import type { Entry, Group, Level, LogResponse, StoreStats } from '../lib/types'
  import Histogram from '../components/Histogram.svelte'
  import Icon from '../components/Icon.svelte'
  import Spark from '../components/Spark.svelte'

  const L = app.logs
  const LEVELS: Level[] = ['ERROR', 'WARN', 'INFO', 'DEBUG']
  const COL: Record<Level, string> = { ERROR: 'var(--err)', WARN: 'var(--warn)', INFO: 'var(--info)', DEBUG: 'var(--dbg)' }

  let res = $state<LogResponse | null>(null)
  let shown = $state({ from: 0, to: 0 })
  let loading = $state(false)
  let error = $state<string | null>(null)
  let stats = $state<StoreStats | null>(null)
  let qInput = $state(L.q)
  let search: HTMLInputElement | undefined = $state()
  let seq = 0

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
    } catch (e) {
      if (mine === seq) error = app.describe(e)
    } finally {
      if (mine === seq) loading = false
    }
  }

  // Reload when a filter changes (debounced), and keep a preset range moving.
  $effect(() => {
    void [app.activeId, L.stack, L.service, L.q, L.preset, L.from, L.to, [...L.levels].join()]
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
    // Filters set from elsewhere (a group, a row, the overview) show up in the box.
    qInput = L.q
  })

  function preset(p: Preset) {
    L.setPreset(p)
  }

  function toggleLevel(l: Level) {
    if (L.levels.has(l)) L.levels.delete(l)
    else L.levels.add(l)
  }

  function toggleLive() {
    L.live = !L.live
    if (L.live && !L.preset) L.setPreset('15m')
  }

  function setQ(q: string) {
    L.q = q
    qInput = q
  }

  let services = $derived.by(() => {
    const list = app.services.filter((s) => L.stack === 'all' || s.stack === L.stack)
    const pinned = list.filter((s) => app.isPinned(s.key))
    const rest = list.filter((s) => !app.isPinned(s.key)).sort((a, b) => a.fullName.localeCompare(b.fullName))
    return { pinned, rest }
  })

  let terms = $derived(
    L.q
      .split(/\s+/)
      .filter((t) => t && !/^[a-z]+:/i.test(t))
      .map((t) => t.toLowerCase()),
  )

  function marks(text: string): { t: string; m: boolean }[] {
    if (!terms.length) return [{ t: text, m: false }]
    const lower = text.toLowerCase()
    const out: { t: string; m: boolean }[] = []
    let i = 0
    while (i < text.length) {
      let best = -1
      let len = 0
      for (const term of terms) {
        const j = lower.indexOf(term, i)
        if (j >= 0 && (best < 0 || j < best)) {
          best = j
          len = term.length
        }
      }
      if (best < 0) break
      if (best > i) out.push({ t: text.slice(i, best), m: false })
      out.push({ t: text.slice(best, best + len), m: true })
      i = best + len
    }
    if (i < text.length) out.push({ t: text.slice(i), m: false })
    return out
  }

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

  function toggleGroup(fp: string) {
    if (L.openGroups.has(fp)) L.openGroups.delete(fp)
    else L.openGroups.add(fp)
  }
  function expandAll() {
    const all = groups.every((g) => L.openGroups.has(g.fp))
    for (const g of groups) {
      if (all) L.openGroups.delete(g.fp)
      else L.openGroups.add(g.fp)
    }
  }
  function toggleRow(id: number) {
    if (L.openRows.has(id)) L.openRows.delete(id)
    else L.openRows.add(id)
  }
  function onlyGroup(g: Group) {
    setQ(`fp:${g.fp}`)
    document.getElementById('stream')?.scrollIntoView({ behavior: 'smooth', block: 'start' })
  }
  function context(e: Entry) {
    L.zoom(e.ts / 1000 - 30000, e.ts / 1000 + 30000)
    L.stack = 'all'
    L.service = 'all'
    setQ('')
  }
  async function copy(text: string) {
    try {
      await navigator.clipboard.writeText(text)
      app.notify('Copied to the clipboard.')
    } catch {
      app.notify('The clipboard is not available here.', 'err')
    }
  }

  function keydown(e: KeyboardEvent) {
    const tag = (document.activeElement as HTMLElement | null)?.tagName
    if (e.key === '/' && tag !== 'INPUT' && tag !== 'TEXTAREA' && tag !== 'SELECT') {
      e.preventDefault()
      search?.focus()
    }
  }

  const short = (id: string) => (id.length > 12 ? id.slice(0, 12) : id)
</script>

<svelte:window onkeydown={keydown} />

<section class="panel">
  <div class="pb fbar">
    <div class="seg" role="group" aria-label="Time range">
      {#each Object.keys(PRESETS) as p (p)}
        <button aria-pressed={L.preset === p} onclick={() => preset(p as Preset)}>{p}</button>
      {/each}
    </div>
    <select class="ctl" aria-label="Stack" value={L.stack} onchange={(e) => { L.stack = (e.target as HTMLSelectElement).value; L.service = 'all' }}>
      <option value="all">All stacks</option>
      {#each app.snap?.stacks ?? [] as s (s.name)}<option value={s.name}>{s.name}</option>{/each}
    </select>
    <select class="ctl" aria-label="Service" value={L.service} onchange={(e) => (L.service = (e.target as HTMLSelectElement).value)}>
      <option value="all">All services</option>
      {#if services.pinned.length}
        <optgroup label="★ Pinned">
          {#each services.pinned as s (s.key)}<option value={s.fullName}>{s.fullName}</option>{/each}
        </optgroup>
        <optgroup label="Services">
          {#each services.rest as s (s.key)}<option value={s.fullName}>{s.fullName}</option>{/each}
        </optgroup>
      {:else}
        {#each services.rest as s (s.key)}<option value={s.fullName}>{s.fullName}</option>{/each}
      {/if}
    </select>
    {#each LEVELS as l (l)}
      <button class="lv" aria-pressed={L.levels.has(l)} onclick={() => toggleLevel(l)}>
        <span class="sw" style:background={COL[l]}></span>{l[0] + l.slice(1).toLowerCase()}<span class="n">{res?.result.levels[l] ?? 0}</span>
      </button>
    {/each}
    <label class="search">
      <Icon name="search" size={14} />
      <input bind:this={search} bind:value={qInput} placeholder="Search text, or  service:api  level:error  replica:2" autocomplete="off" spellcheck="false" />
      {#if qInput}<button class="btn ghost" style="height:22px;padding:0 6px" onclick={() => setQ('')} aria-label="Clear search">✕</button>{:else}<kbd class="muted mono hide-sm" style="font-size:11px">/</kbd>{/if}
    </label>
    <button class="ctl" class:live={L.live} aria-pressed={L.live} onclick={toggleLive}><span class="live-dot"></span>Live</button>
    <button class="ctl" onclick={() => (app.drawer = { kind: 'download' })}><Icon name="download" size={14} />Download</button>
  </div>
  <div class="ph" style="border-top:1px solid var(--line);border-bottom:0;padding-bottom:0">
    <h2>Log volume</h2>
    <span class="muted mono" style="font-size:12px">
      {#if L.preset}last {L.preset}{:else}{hhmm(L.from)} – {hhmm(L.to)}
        <button class="btn ghost" style="height:22px;font-size:12px" onclick={() => preset('3h')}>Reset zoom</button>{/if}
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
      <span class="spacer"></span>
      {#if groups.length}<button class="btn ghost" onclick={expandAll}>{groups.every((g) => L.openGroups.has(g.fp)) ? 'Collapse all' : 'Expand all'}</button>{/if}
    </div>
    {#each groups as g (g.fp)}
      {@const open = L.openGroups.has(g.fp)}
      <div class="grp" class:open>
        <button aria-expanded={open} onclick={() => toggleGroup(g.fp)}>
          <Icon name="chev" size={12} class="chev" />
          <span class="count" class:w={g.level === 'WARN'}>{g.count}</span>
          <span>
            <span class="ttl">{#each patternParts(g.pattern || firstLine(g.sample)) as part, i (i)}{#if part.v}<var>{part.text}</var>{:else}{part.text}{/if}{/each}</span>
            <span class="meta">
              <span class="pill acc" style="font-size:11px">{g.services.join(', ')}</span>
              {#if g.exc}<span style="color:var(--exc)">exception</span>{:else}<span>{g.level === 'WARN' ? 'warning' : 'log error'}</span>{/if}
              <span>last {ago(g.last / 1000, app.now)}</span>
              <span class="spacer"></span>
              <Spark values={g.spark} />
            </span>
          </span>
        </button>
        {#if open}
          <div class="body">
            {@render trace(g.sample)}
            <dl class="kv">
              <dt>First seen</dt><dd>{hms(g.first / 1000)} ({ago(g.first / 1000, app.now)})</dd>
              <dt>Last seen</dt><dd>{hms(g.last / 1000)} ({ago(g.last / 1000, app.now)})</dd>
              <dt>Nodes</dt><dd>{g.nodes.join(', ')}</dd>
              <dt>Replicas</dt><dd>{g.sources} distinct</dd>
              <dt>Fingerprint</dt><dd>{g.fp}</dd>
            </dl>
            <div style="display:flex;gap:8px;flex-wrap:wrap">
              <button class="btn" onclick={() => onlyGroup(g)}>Show these {g.count} logs</button>
              <button class="btn ghost" onclick={() => copy(g.sample)}>Copy {g.exc ? 'stack trace' : 'message'}</button>
            </div>
          </div>
        {/if}
      </div>
    {:else}
      <div class="empty">{res ? 'No errors match these filters.' : ''}</div>
    {/each}
  </section>

  <section class="panel" id="stream">
    <div class="ph">
      <h2>Log stream</h2>
      {#if res}<span class="muted" style="font-size:12px">{res.result.total.toLocaleString()} lines{#if res.result.total > res.result.entries.length} · newest {res.result.entries.length}{/if}</span>{/if}
      <span class="spacer"></span>
      <label class="toggle"><input type="checkbox" bind:checked={L.collapse} />Collapse repeats</label>
    </div>
    <div class="stream">
      {#each rows as { e, n } (e.id)}
        {@const open = L.openRows.has(e.id)}
        <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
        <div class="row {e.level}" class:open onclick={(ev) => { if (!(ev.target as HTMLElement).closest('.detail')) toggleRow(e.id) }}>
          <span class="ts">{hms(e.ts / 1000).slice(0, 12)}</span>
          <span class="lvl {e.level}">{e.level}</span>
          <span class="svc" title="service.replica">{e.service}<span class="muted">.{e.replica}</span></span>
          <span class="msg">{#each marks(open ? e.text : tidy(firstLine(e.text))) as m, i (i)}{#if m.m}<mark>{m.t}</mark>{:else}{m.t}{/if}{/each}{#if e.lines > 1 && !open}<span class="rep" title="Multi-line entry, reassembled per replica">{e.lines} lines</span>{/if}{#if n > 1}<span class="rep">+{n - 1} similar</span>{/if}</span>
          {#if open}
            <div class="detail">
              {#if e.raw}<details><summary class="muted" style="cursor:pointer;font-size:12px">Original JSON line</summary><pre class="trace raw">{e.raw}</pre></details>{/if}
              <dl class="kv">
                <dt>time</dt><dd>{new Date(e.ts / 1000).toISOString()}</dd>
                <dt>service</dt><dd>{e.service}</dd>
                <dt>replica</dt><dd>{e.replica}</dd>
                <dt>node</dt><dd>{e.node}</dd>
                <dt>container</dt><dd>{short(e.source)}</dd>
                <dt>stream</dt><dd>{e.stream}</dd>
              </dl>
              <div style="display:flex;gap:8px;flex-wrap:wrap;font-family:var(--sans)">
                <button class="btn" onclick={() => { L.service = e.service; setQ('') }}>Only this service</button>
                <button class="btn" onclick={() => { L.service = e.service; setQ(`replica:${e.replica}`) }}>Only this replica</button>
                <button class="btn ghost" onclick={() => context(e)}>Show ±30s around it</button>
                <button class="btn ghost" onclick={() => copy(e.text)}>Copy</button>
              </div>
            </div>
          {/if}
        </div>
      {:else}
        <div class="empty">{res ? 'No log lines match. Widen the time range or clear the search.' : ''}</div>
      {/each}
    </div>
  </section>
</div>

{#snippet trace(text: string)}
  {@const lines = text.split('\n')}
  <pre class="trace">{#each lines as l, i (i)}{#if i === 0}<b>{l}</b>{:else}{'\n'}<span class="fr">{l}</span>{/if}{/each}</pre>
{/snippet}
