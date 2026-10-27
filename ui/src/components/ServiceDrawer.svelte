<script lang="ts">
  // A service with its logs and errors in tabs, so you can see why it fails without leaving the page.
  import { SvelteSet } from 'svelte/reactivity'
  import { app, LEVELS, type ServiceTab } from '../lib/state.svelte'
  import { api, query } from '../lib/api'
  import { ago, bytes, firstLine, hhmm, patternParts, pct, tidy } from '../lib/format'
  import { svcState, taskPill } from '../lib/health'
  import type { Level, LogResponse } from '../lib/types'
  import Drawer from './Drawer.svelte'
  import Histogram from './Histogram.svelte'
  import Icon from './Icon.svelte'
  import LogRow from './LogRow.svelte'
  import PinButton from './PinButton.svelte'
  import Spark from './Spark.svelte'

  let { key, tab }: { key: string; tab: ServiceTab } = $props()
  let s = $derived(app.services.find((x) => x.key === key))
  let failing = $derived(!!s && s.running === 0 && s.desired > 0)
  let errs = $derived(s ? (app.errors?.byService[s.fullName] ?? 0) : 0)

  const COL: Record<Level, string> = { ERROR: 'var(--err)', WARN: 'var(--warn)', INFO: 'var(--info)', DEBUG: 'var(--dbg)' }
  const levels = new SvelteSet<Level>(LEVELS)
  const openRows = new SvelteSet<number>()
  let replica = $state<string | null>(null)

  let hist = $state<LogResponse | null>(null)
  let lines = $state<LogResponse | null>(null)
  let win = $state({ from: 0, to: 0 })

  /** The stored range, at most the last 6 hours for the chart and 24 hours for lines. */
  function range(max: number) {
    const to = Date.now()
    const oldest = app.errors?.oldest ? app.errors.oldest / 1000 : 0
    return { from: Math.floor(Math.max(to - max, Math.min(oldest, to - 15 * 60e3))), to }
  }

  // Errors and groups: for the overview and errors tabs.
  $effect(() => {
    const name = s?.fullName
    void app.snap?.fetchedAt
    if (!name) return
    const r = range(6 * 3600e3)
    api<LogResponse>(`/c/${encodeURIComponent(app.activeId)}/logs?${query({ ...r, service: name, levels: 'ERROR,WARN', buckets: 36, limit: 1 })}`)
      .then((x) => {
        hist = x
        win = r
      })
      .catch(() => {})
  })

  // Lines: the logs tab, and the newest line for a failing service.
  $effect(() => {
    const name = s?.fullName
    if (!name || (tab !== 'logs' && !failing)) return
    const lv = tab === 'logs' ? [...levels].join(',') : LEVELS.join(',')
    const q = tab === 'logs' && replica ? `replica:${replica}` : ''
    const get = () => {
      const r = range(24 * 3600e3)
      api<LogResponse>(`/c/${encodeURIComponent(app.activeId)}/logs?${query({ ...r, service: name, levels: lv, q, buckets: 1, limit: tab === 'logs' ? 150 : 1 })}`)
        .then((x) => (lines = x))
        .catch(() => {})
    }
    get()
    const t = setInterval(get, Math.max(2, app.settings?.logPollSecs ?? 3) * 1000)
    return () => clearInterval(t)
  })

  let newest = $derived(lines?.result.entries[0])
  let groups = $derived(hist?.result.groups ?? [])

  function setTab(t: ServiceTab) {
    if (app.drawer?.kind === 'service') app.drawer = { ...app.drawer, tab: t }
  }
  function step(d: number) {
    const list = app.services
    const i = list.findIndex((x) => x.key === key)
    if (i < 0 || !list.length) return
    const next = list[(i + d + list.length) % list.length]
    replica = null
    openRows.clear()
    lines = null
    hist = null
    app.drawer = { kind: 'service', key: next.key, tab }
  }
  function replicaLogs(slot: string) {
    replica = slot
    setTab('logs')
  }
  function pickLevel(l: Level, add: boolean) {
    if (add) {
      if (levels.has(l)) levels.delete(l)
      else levels.add(l)
      if (!levels.size) LEVELS.forEach((x) => levels.add(x))
      return
    }
    const solo = levels.size === 1 && levels.has(l)
    levels.clear()
    for (const x of solo ? LEVELS : [l]) levels.add(x)
  }
  function keydown(e: KeyboardEvent) {
    const tag = (document.activeElement as HTMLElement | null)?.tagName
    if (tag === 'INPUT' || tag === 'TEXTAREA' || app.palette) return
    if (e.key === '[' || e.key === ']') step(e.key === ']' ? 1 : -1)
  }
</script>

<svelte:window onkeydown={keydown} />

<Drawer title={s ? s.fullName : key}>
  {#snippet actions()}
    <PinButton {key} />
    <button class="ib" title="Previous service ( [ )" aria-label="Previous service" onclick={() => step(-1)}><Icon name="chevL" size={16} /></button>
    <button class="ib" title="Next service ( ] )" aria-label="Next service" onclick={() => step(1)}><Icon name="chev" size={16} /></button>
  {/snippet}
  {#snippet sub()}
    {#if s}
      {@const [cls] = svcState(s)}
      <span class="pill {cls}">{s.running}/{s.desired}</span><span class="pill">{s.mode}</span><span class="kind">{s.kind}</span>
      {#if s.updatedAt}<span class="muted">updated {ago(s.updatedAt, app.now)}</span>{/if}
    {/if}
  {/snippet}
  {#snippet tabs()}
    {#each [['overview', 'Overview'], ['logs', 'Logs'], ['errors', 'Errors']] as [k, label] (k)}
      <button role="tab" aria-selected={tab === k} onclick={() => setTab(k as ServiceTab)}>{label}{#if k === 'errors' && errs}<span class="n">{errs}</span>{/if}</button>
    {/each}
  {/snippet}
  {#snippet foot()}
    <span>Read-only. Nothing here can restart or change this service.</span>
    <span class="spacer"></span>
    {#if s}<button class="btn pri sm" onclick={() => app.showLogs([s!.fullName])}><Icon name="external" size={13} />Open in Logs</button>{/if}
  {/snippet}

  {#if !s}
    <div class="empty">This service is no longer running on {app.active?.name}.</div>
  {:else if tab === 'overview'}
    {#if failing}
      {@const t = s.tasks.find((t) => t.state !== 'running')}
      {@const fails = s.tasks.reduce((a, t) => a + t.failures, 0)}
      <div class="callout">
        <div class="h"><Icon name="x" size={14} />Nothing running{#if t?.exitCode} · exit {t.exitCode}{/if}{#if fails} · {fails} earlier {fails === 1 ? 'task' : 'tasks'} failed{/if}</div>
        {#if newest}
          <pre>{tidy(firstLine(newest.text))}</pre>
          <span class="muted" style="font-size:12px">Newest line, from replica {newest.replica}, {ago(newest.ts / 1000, app.now)}.{#if t?.error} Docker says: {t.error}{/if}</span>
        {:else if t?.error || t?.message}
          <pre>{t.error || t.message}</pre>
        {/if}
        <div><button class="btn sm" onclick={() => setTab('logs')}>All output of this service</button></div>
      </div>
    {/if}
    {#if groups.length && !failing}
      {@const g = groups[0]}
      <div>
        <h4>Top error</h4>
        <button class="grp boxed" onclick={() => setTab('errors')}>
          <span class="count" class:w={g.level === 'WARN'}>{g.count}</span>
          <span class="ttl">{g.pattern || firstLine(g.sample)}</span>
          <Spark values={g.spark} warn={g.level === 'WARN'} />
          <span class="meta">last {ago(g.last / 1000, app.now)}{#if groups.length > 1} · {groups.length - 1} more {groups.length === 2 ? 'group' : 'groups'}{/if}</span>
        </button>
      </div>
    {/if}
    <dl class="kv">
      <dt>Image</dt><dd>{s.image}</dd>
      <dt>Ports</dt><dd>{s.ports.join(', ') || '–'}</dd>
      <dt>Stack</dt><dd>{s.stack}</dd>
      <dt>Usage</dt><dd>CPU {pct(s.cpuPercent, 1)} · memory {bytes(s.memUsed)}</dd>
      {#if s.kind === 'swarm'}<dt>Service ID</dt><dd>{s.id.slice(0, 12)}</dd>{/if}
    </dl>
    <div>
      <h4>{s.kind === 'swarm' ? 'Tasks' : 'Containers'}</h4>
      <div class="tw boxed">
        <table>
          <thead><tr><th>Replica</th><th>Node</th><th>State</th><th>Since</th><th class="hide-sm">CPU / Mem</th><th class="actcell"><span class="sr">Logs</span></th></tr></thead>
          <tbody>
            {#each s.tasks as t (t.id)}
              {@const [pc, label] = taskPill(t)}
              <tr>
                <td class="mono">{t.slot}</td>
                <td class="mono">{t.node}</td>
                <td>
                  <span class="pill {pc}">{label}</span>{#if t.exitCode}<span class="muted mono" style="font-size:11.5px"> exit {t.exitCode}</span>{/if}
                  {#if t.failures}<span class="muted" style="font-size:11.5px;white-space:nowrap"> · {t.failures} failed before</span>{/if}
                  {#if t.error || (t.state !== 'running' && t.message)}<div class="muted" style="font-size:12px;margin-top:3px;white-space:pre-wrap">{t.error || t.message}</div>{/if}
                </td>
                <td class="muted" style="white-space:nowrap">{t.since ? ago(t.since, app.now) : '–'}</td>
                <td class="hide-sm mono" style="white-space:nowrap">{t.cpuPercent == null && t.memUsed == null ? '–' : `${pct(t.cpuPercent, 1)} · ${bytes(t.memUsed)}`}</td>
                <td class="actcell"><button class="ib" title="Logs of replica {t.slot}" aria-label="Logs of replica {t.slot}" onclick={() => replicaLogs(t.slot)}><Icon name="logs" size={14} /></button></td>
              </tr>
            {:else}
              <tr><td colspan="6" class="muted">No tasks.</td></tr>
            {/each}
          </tbody>
        </table>
      </div>
    </div>
    <div>
      <h4>Errors &amp; warnings · {win.to - win.from > 5.9 * 3600e3 ? 'last 6h' : `since ${hhmm(win.from)}`}</h4>
      {#if hist}<div class="boxed"><Histogram buckets={hist.result.histogram} from={win.from} to={win.to} levels={['WARN', 'ERROR']} height={110} /></div>{/if}
    </div>
  {:else if tab === 'logs'}
    <div class="fbar">
      <span class="lvgroup" role="group" aria-label="Levels">
        {#each LEVELS as l (l)}
          <button class="lv" aria-pressed={levels.has(l)} onclick={(e) => pickLevel(l, e.shiftKey || e.metaKey || e.ctrlKey)} title="Click: only {l.toLowerCase()}. Shift-click: add or remove.">
            <span class="sw" style:background={COL[l]}></span>{l[0] + l.slice(1).toLowerCase()}<span class="n">{lines?.result.levels[l] ?? 0}</span>
          </button>
        {/each}
      </span>
      {#if replica}<button class="fchip" onclick={() => (replica = null)} aria-label="All replicas"><span class="t">replica {replica}</span><span class="x"><Icon name="x" size={11} /></span></button>{/if}
      <span class="spacer"></span>
      <span class="muted" style="font-size:12px;display:inline-flex;gap:6px;align-items:center"><span class="live-dot" style="background:var(--err)"></span>live</span>
    </div>
    <div class="boxed">
      <div class="stream">
        {#each lines?.result.entries ?? [] as e (e.id)}
          <LogRow {e} service={false} open={openRows.has(e.id)} ontoggle={() => (openRows.has(e.id) ? openRows.delete(e.id) : openRows.add(e.id))} />
        {:else}
          <div class="empty">{lines ? 'No lines stored for this service yet.' : ''}</div>
        {/each}
      </div>
    </div>
    {#if lines && lines.result.total > lines.result.entries.length}
      <span class="muted" style="font-size:12px">Newest {lines.result.entries.length} of {lines.result.total.toLocaleString()} lines. Open in Logs for search, groups and download.</span>
    {/if}
  {:else}
    {#each groups as g (g.fp)}
      <div class="boxed" style="padding:12px 14px;display:grid;gap:8px">
        <div style="display:flex;gap:10px;align-items:center;min-width:0">
          <span class="grp" style="all:unset"><span class="count" class:w={g.level === 'WARN'} style="font-family:var(--mono);font-weight:700;font-size:12px;padding:2px 8px;border-radius:7px">{g.count}</span></span>
          <b class="mono" style="font-size:12px;font-weight:600;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap" title={g.pattern}>{#each patternParts(g.pattern || firstLine(g.sample)) as part, i (i)}{part.text}{/each}</b>
          <span class="spacer"></span><Spark values={g.spark} warn={g.level === 'WARN'} />
        </div>
        <span class="muted" style="font-size:12px">{g.exc ? 'exception' : g.level === 'WARN' ? 'warning' : 'log error'} · first {ago(g.first / 1000, app.now)} · last {ago(g.last / 1000, app.now)} · {g.sources} {g.sources === 1 ? 'container' : 'containers'}</span>
        <pre class="trace" class:plain={g.level !== 'ERROR'}>{#each g.sample.split('\n') as l, i (i)}{#if i === 0}<b>{l}</b>{:else}{'\n'}<span class="fr">{l}</span>{/if}{/each}</pre>
        <div style="display:flex;gap:8px;flex-wrap:wrap">
          <button class="btn sm" onclick={() => { app.showGroup(g.fp); app.logs.sources.add(s!.fullName) }}>Show all {g.count} in Logs</button>
          <button class="btn ghost sm" onclick={() => app.copy(g.sample)}><Icon name="copy" size={13} />Copy {g.exc ? 'stack trace' : 'message'}</button>
        </div>
      </div>
    {:else}
      <div class="empty">{hist ? `No errors or warnings from this service since ${hhmm(win.from)}.` : ''}</div>
    {/each}
  {/if}
</Drawer>
