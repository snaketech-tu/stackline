<script lang="ts">
  // A service with its logs and errors in tabs, so you can see why it fails without leaving the page.
  import { untrack } from 'svelte'
  import { app, LEVELS, LINES, LogFilters, load, save, type ServiceTab } from '../lib/state.svelte'
  import { api, apiFetch, fetchEvents, fetchLogs, keepEntries, query } from '../lib/api'
  import { eventColor, eventLabel, isCrash } from '../lib/events'
  import { ago, bytes, dateTime, firstLine, hhmm, patternParts, pct, searchTerms, tidy } from '../lib/format'
  import { svcState, taskPill } from '../lib/health'
  import type { DockerEvent, Level, LogResponse, ServiceDetails } from '../lib/types'
  import Drawer from './Drawer.svelte'
  import Histogram from './Histogram.svelte'
  import Icon from './Icon.svelte'
  import LogRow from './LogRow.svelte'
  import PinButton from './PinButton.svelte'
  import CollectSwitch from './CollectSwitch.svelte'
  import Spark from './Spark.svelte'

  let { key, tab }: { key: string; tab: ServiceTab } = $props()
  let s = $derived(app.services.find((x) => x.key === key))
  // A plain string, so effects that read it do not rerun on every snapshot (each one brings new service objects).
  let name = $derived(s?.fullName)
  let failing = $derived(!!s && s.running === 0 && s.desired > 0)
  let errs = $derived(s ? (app.errors?.byService[s.fullName] ?? 0) : 0)

  const COL: Record<Level, string> = { ERROR: 'var(--err)', WARN: 'var(--warn)', INFO: 'var(--info)', DEBUG: 'var(--dbg)' }
  /** Levels, open entries and line count of the Logs tab; the same controls as the Logs page. */
  const F = new LogFilters()
  let replica = $state<string | null>(null)
  let qInput = $state('')
  let q = $state('')
  /** Covers the whole window instead of most of it. */
  let full = $state(load('stackline-drawer-full') === '1')
  function toggleFull() {
    full = !full
    save('stackline-drawer-full', full ? '1' : '0')
  }
  $effect(() => {
    const v = qInput
    const t = setTimeout(() => (q = v.trim()), 250)
    return () => clearTimeout(t)
  })

  /** Copies a Markdown report of this service: configuration, errors, crashes and the lines before the last one. */
  let copying = $state(false)
  async function copyReport() {
    if (!name || copying) return
    copying = true
    const text = apiFetch(`/c/${encodeURIComponent(app.activeId)}/report?${query({ name })}`).then((r) => r.text())
    try {
      // Handing the clipboard a promise keeps the click's permission while the report is built (Safari needs that).
      if (typeof ClipboardItem !== 'undefined' && navigator.clipboard?.write) {
        await navigator.clipboard.write([new ClipboardItem({ 'text/plain': text.then((t) => new Blob([t], { type: 'text/plain' })) })])
      } else {
        await navigator.clipboard.writeText(await text)
      }
      app.notify('Debug report copied as Markdown. Paste it into a ticket or a chat.')
    } catch (e) {
      const failed = await text.then(() => null, (err) => err)
      app.notify(failed ? `Could not build the report: ${app.describe(failed)}` : 'The clipboard is not available here.', 'err')
    } finally {
      copying = false
    }
  }

  // The full configuration, read from Docker when the details open and every 30 seconds after.
  let details = $state<ServiceDetails | null>(null)
  let detailsError = $state<string | null>(null)
  $effect(() => {
    const id = app.activeId
    if (!name) return
    const get = () =>
      api<ServiceDetails>(`/c/${encodeURIComponent(id)}/service?${query({ name })}`)
        .then((d) => {
          details = d
          detailsError = null
        })
        .catch((e) => (detailsError = app.describe(e)))
    get()
    const t = setInterval(get, 30000)
    return () => clearInterval(t)
  })

  let hist = $state.raw<LogResponse | null>(null)
  /** Starts, stops and crashes of the last 24 hours. */
  let events = $state.raw<DockerEvent[]>([])
  let lines = $state.raw<LogResponse | null>(null)
  let win = $state({ from: 0, to: 0 })

  /** The stored range, at most the last 6 hours for the chart and 24 hours for lines. */
  const range = (max: number) => app.storedRange(max)

  // Errors and groups: for the overview and errors tabs.
  $effect(() => {
    void app.snap?.fetchedAt
    if (!name) return
    const r = range(6 * 3600e3)
    fetchLogs(app.activeId, { ...r, service: name, levels: 'ERROR,WARN', buckets: 36, limit: 1 })
      .then((x) => {
        hist = x
        win = r
      })
      .catch(() => {})
    const now = Date.now()
    fetchEvents(app.activeId, { from: now - 24 * 3600e3, to: now + 60e3, service: name, limit: 200 })
      .then((x) => (events = x))
      .catch(() => {})
  })
  // Starts alone are noise: shown are the ones that say something went wrong or changed.
  let notable = $derived(events.filter((e) => e.kind !== 'start').reverse())
  let crashes = $derived(events.filter(isCrash).length)
  const when = (ts: number) => (app.now - ts / 1000 > 20 * 3600e3 ? dateTime(ts / 1000) : hhmm(ts / 1000))

  // Lines: the logs tab, and the newest line for a failing service.
  let linesSeq = 0
  $effect(() => {
    const svc = name
    if (!svc || (tab !== 'logs' && !failing)) return
    const id = app.activeId
    const lv = tab === 'logs' ? [...F.levels].join(',') : LEVELS.join(',')
    const filter = tab === 'logs' ? [replica ? `replica:${replica}` : '', q].filter(Boolean).join(' ') : ''
    const n = tab === 'logs' ? F.limit : 1
    const mine = ++linesSeq
    const get = () => {
      fetchLogs(id, { ...range(24 * 3600e3), service: svc, levels: lv, q: filter, buckets: 1, limit: n })
        // A slow answer for filters that changed since must not replace the current one.
        .then((x) => {
          if (mine !== linesSeq) return
          lines = keepEntries(lines, x)
        })
        .catch(() => {})
    }
    untrack(get)
    // Big pages refresh less often; the newest lines still show up within 15 seconds.
    const every = n > 500 ? 15000 : Math.max(2, untrack(() => app.settings?.logPollSecs) ?? 3) * 1000
    const t = setInterval(() => !document.hidden && get(), every)
    return () => clearInterval(t)
  })
  let terms = $derived(searchTerms(q))

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
    F.openRows.clear()
    F.limit = LINES.first
    lines = null
    hist = null
    events = []
    details = null
    app.drawer = { kind: 'service', key: next.key, tab }
  }
  function replicaLogs(slot: string) {
    replica = slot
    setTab('logs')
  }
  function keydown(e: KeyboardEvent) {
    const tag = (document.activeElement as HTMLElement | null)?.tagName
    if (tag === 'INPUT' || tag === 'TEXTAREA' || app.palette) return
    if (e.key === '[' || e.key === ']') step(e.key === ']' ? 1 : -1)
  }
</script>

<svelte:window onkeydown={keydown} />

<Drawer title={s ? s.fullName : key} size={full ? 'full' : 'wide'}>
  {#snippet actions()}
    <button class="btn sm ghost" title="Copy a debug report of this service as Markdown: configuration, error groups, crashes and the lines before the last one. Environment values are left out." onclick={copyReport} disabled={copying || !s}><Icon name="copy" size={14} /><span class="hide-sm">{copying ? 'Copying…' : 'Debug report'}</span></button>
    <PinButton {key} />
    <button class="ib hide-sm" title={full ? 'Leave some of the page visible' : 'Use the whole window'} aria-label={full ? 'Smaller' : 'Use the whole window'} onclick={toggleFull}><Icon name={full ? 'minimize' : 'maximize'} size={15} /></button>
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
      <button role="tab" aria-selected={tab === k} onclick={() => setTab(k as ServiceTab)}>{label}{#if k === 'errors' && errs}<span class="n" title="Errors in the last hour">{errs.toLocaleString()}</span>{/if}</button>
    {/each}
  {/snippet}
  {#snippet foot()}
    <span>Read-only. Nothing here can restart or change this service.</span>
    <span class="spacer"></span>
    {#if s}<span class="toggle" title="Read and store the logs of this service">Collect logs<CollectSwitch names={[s.fullName]} label={s.fullName} /></span>{/if}
    {#if s}<button class="btn pri sm" onclick={() => app.showLogs([s!.fullName])}><Icon name="external" size={13} />Open in Logs</button>{/if}
  {/snippet}

  {#if s && tab !== 'overview' && !app.collects(s.fullName)}
    <div class="callout info">
      <div class="h"><Icon name="logs" size={14} />The logs of this service are not collected</div>
      <span class="muted" style="font-size:12.5px">Stackline does not read new lines from it. {lines?.result.total ? 'The lines below were stored before.' : ''}</span>
      <div style="display:flex;gap:6px;flex-wrap:wrap">
        <button class="btn sm" onclick={() => app.setCollects([s!.fullName], true)}>Collect from now on</button>
        {#if lines?.result.total}<button class="btn ghost sm" onclick={() => app.clearService(s!.fullName).catch((e) => app.notify(app.describe(e), 'err'))}>Delete stored lines</button>{/if}
      </div>
    </div>
  {/if}
  {#if !s}
    <div class="empty">This service is no longer running on {app.active?.name}.</div>
  {:else if tab === 'overview'}
    {#if failing}
      {@const t = s.tasks.find((t) => t.state !== 'running')}
      {@const fails = s.tasks.reduce((a, t) => a + t.failures, 0)}
      <div class="callout">
        <div class="h"><Icon name="x" size={14} />Nothing running{#if t?.exitCode}{' · '}exit {t.exitCode}{/if}{#if fails}{' · '}{fails} earlier {fails === 1 ? 'task' : 'tasks'} failed{/if}</div>
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
          <span class="meta">last {ago(g.last / 1000, app.now)}{#if groups.length > 1}{' · '}{groups.length - 1} more {groups.length === 2 ? 'group' : 'groups'}{/if}</span>
        </button>
      </div>
    {/if}
    {#if notable.length}
      <div>
        <h4>Stops and crashes · last 24 h{#if crashes}{' · '}<span style="color:var(--err)">{crashes} {crashes === 1 ? 'crash' : 'crashes'}</span>{/if}</h4>
        <div class="tw boxed">
          <table class="evlist">
            <tbody>
              {#each notable.slice(0, 8) as e (e.ts + e.source + e.kind)}
                <tr>
                  <td class="mono muted" style="white-space:nowrap" title={dateTime(e.ts / 1000)}>{when(e.ts)}</td>
                  <td class="mono" style="white-space:nowrap">#{e.replica}</td>
                  <td style:color={eventColor(e) === 'var(--muted)' ? undefined : eventColor(e)}>{eventLabel(e)}</td>
                  <td class="actcell"><button class="btn ghost sm" style="white-space:nowrap" onclick={() => app.showEvent(e)} title="The lines replica {e.replica} printed in the 2 minutes before">Lines before</button></td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
        {#if notable.length > 8}<span class="muted" style="font-size:12px">and {notable.length - 8} earlier</span>{/if}
      </div>
    {/if}
    {@const d = details}
    <div class="dsec">
      <h4>Service</h4>
      <dl class="kv">
        <dt>Image</dt><dd>{d?.image || s.image}</dd>
        <dt>Ports</dt><dd>{(d?.ports.length ? d.ports : s.ports).join(', ') || '–'}</dd>
        <dt>Stack</dt><dd>{s.stack} · {s.mode}{#if s.kind === 'swarm'}{' · '}{s.desired} {s.desired === 1 ? 'replica' : 'replicas'}{/if}</dd>
        <dt>Usage</dt><dd>CPU {pct(s.cpuPercent, 1)} · memory {bytes(s.memUsed)}</dd>
        {#if d?.limits}<dt>Limits</dt><dd>{d.limits}</dd>{/if}
        {#if d?.reservations}<dt>Reserved</dt><dd>{d.reservations}</dd>{/if}
        {#if d?.created}<dt>Created</dt><dd>{dateTime(d.created)} · {ago(d.created, app.now)}</dd>{/if}
        {#if d?.updated}<dt>Updated</dt><dd>{dateTime(d.updated)} · {ago(d.updated, app.now)}</dd>{/if}
        {#if d?.started}<dt>Started</dt><dd>{dateTime(d.started)}{#if s.running}{' · '}up {ago(d.started, app.now).replace(' ago', '')}{/if}{#if d.container}{' · '}{d.container}{/if}</dd>{/if}
        {#if d?.restart}<dt>Restart policy</dt><dd>{d.restart}</dd>{/if}
        {#if d?.restartCount != null}<dt>Restarts</dt><dd style:color={d.restartCount ? 'var(--warn)' : undefined}>{d.restartCount}{#if d.oomKilled}{' · '}<span style="color:var(--err)">last exit was out of memory</span>{/if}</dd>{/if}
        {#if d?.stopGrace}<dt>Stop grace</dt><dd>{d.stopGrace}</dd>{/if}
        {#if d?.logDriver}<dt>Log driver</dt><dd>{d.logDriver}</dd>{/if}
        {#if s.kind === 'swarm'}<dt>Service ID</dt><dd>{s.id.slice(0, 12)}</dd>{/if}
      </dl>
      {#if detailsError && !d}<span class="muted" style="font-size:12px">More details could not be read: {detailsError}</span>{/if}
    </div>
    {#if d && (d.entrypoint || d.command || d.workdir || d.user)}
      <div class="dsec">
        <h4>Command</h4>
        <dl class="kv">
          {#if d.entrypoint}<dt>Entrypoint</dt><dd class="wrap">{d.entrypoint}</dd>{/if}
          {#if d.command}<dt>Command</dt><dd class="wrap">{d.command}</dd>{/if}
          {#if d.workdir}<dt>Working dir</dt><dd>{d.workdir}</dd>{/if}
          {#if d.user}<dt>User</dt><dd>{d.user}</dd>{/if}
          {#if d.hostname}<dt>Hostname</dt><dd>{d.hostname}</dd>{/if}
        </dl>
      </div>
    {/if}
    {#if d?.healthcheck}
      <div class="dsec">
        <h4>Health check</h4>
        <dl class="kv"><dt>Runs</dt><dd class="wrap">{d.healthcheck}</dd></dl>
        {#if d.healthLog.length}
          <div class="hlog">
            {#each d.healthLog as h, i (i)}
              <div><span class="muted mono">{h.at ? hhmm(h.at) : '–'}</span><span class="pill {h.exitCode === 0 ? 'ok' : 'err'}">{h.exitCode === 0 ? 'ok' : `exit ${h.exitCode}`}</span><pre>{h.output || '(no output)'}</pre></div>
            {/each}
          </div>
        {/if}
      </div>
    {/if}
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
    {#if details}
      {@const d = details}
      {#if d.networks.length || d.mounts.length}
        <div class="dsec">
          <h4>Networks and storage</h4>
          <dl class="kv">
            {#each d.networks as n (n.name)}<dt>Network</dt><dd>{n.name}{#if n.ip}{' · '}{n.ip}{/if}{#if n.aliases.length}{' · '}aliases {n.aliases.join(', ')}{/if}</dd>{/each}
            {#each d.mounts as m (m.target)}<dt>{m.kind}</dt><dd>{m.source || '(anonymous)'} → {m.target}{#if m.readOnly}{' · '}read-only{/if}</dd>{/each}
          </dl>
        </div>
      {/if}
      {#if s.kind === 'swarm' && (d.updateConfig || d.placement.length || d.secrets.length || d.configs.length)}
        <div class="dsec">
          <h4>Swarm</h4>
          <dl class="kv">
            {#if d.updateConfig}<dt>Updates</dt><dd>{d.updateConfig}</dd>{/if}
            {#if d.rollbackConfig}<dt>Rollbacks</dt><dd>{d.rollbackConfig}</dd>{/if}
            {#if d.placement.length}<dt>Placement</dt><dd>{d.placement.join(' · ')}</dd>{/if}
            {#if d.secrets.length}<dt>Secrets</dt><dd>{d.secrets.join(', ')}</dd>{/if}
            {#if d.configs.length}<dt>Configs</dt><dd>{d.configs.join(', ')}</dd>{/if}
          </dl>
        </div>
      {/if}
      {#if d.env.length}
        <details class="adv dsec">
          <summary><Icon name="chev" size={13} />Environment <span class="muted mono">{d.env.length}</span></summary>
          <dl class="kv" style="margin-top:8px">
            {#each d.env as e (e.name)}<dt class="mono">{e.name}</dt><dd>{#if e.value === null}<span class="hid" title="Looks like a secret, so Stackline does not show it">hidden</span>{:else}{e.value}{/if}</dd>{/each}
          </dl>
        </details>
      {/if}
      {#if d.labels.length}
        <details class="adv dsec">
          <summary><Icon name="chev" size={13} />Labels <span class="muted mono">{d.labels.length}</span></summary>
          <dl class="kv" style="margin-top:8px">
            {#each d.labels as [k, v] (k)}<dt class="mono">{k}</dt><dd>{v}</dd>{/each}
          </dl>
        </details>
      {/if}
    {/if}
    <div>
      <h4>Errors &amp; warnings · {win.to - win.from > 5.9 * 3600e3 ? 'last 6h' : `since ${hhmm(win.from)}`}</h4>
      {#if hist}<div class="boxed"><Histogram buckets={hist.result.histogram} from={win.from} to={win.to} levels={['WARN', 'ERROR']} height={110} marks={events} /></div>{/if}
    </div>
  {:else if tab === 'logs'}
    <div class="fbar">
      <span class="lvgroup" role="group" aria-label="Levels">
        {#each LEVELS as l (l)}
          <button class="lv" aria-pressed={F.levels.has(l)} onclick={(e) => F.pickLevel(l, e.shiftKey || e.metaKey || e.ctrlKey)} title="Click: only {l.toLowerCase()}. Shift-click: add or remove.">
            <span class="sw" style:background={COL[l]}></span>{l[0] + l.slice(1).toLowerCase()}<span class="n">{lines?.result.levels[l] ?? 0}</span>
          </button>
        {/each}
      </span>
      <label class="search">
        <Icon name="search" size={14} />
        <input bind:value={qInput} placeholder="Search this service" autocomplete="off" spellcheck="false" />
        {#if qInput}<button class="ib" style="width:22px;height:22px" onclick={() => (qInput = '')} aria-label="Clear search"><Icon name="x" size={12} /></button>{/if}
      </label>
      {#if replica}<button class="fchip" onclick={() => (replica = null)} aria-label="All replicas"><span class="t">replica {replica}</span><span class="x"><Icon name="x" size={11} /></span></button>{/if}
      <span class="spacer"></span>
      <span class="muted" style="font-size:12px;display:inline-flex;gap:6px;align-items:center"><span class="live-dot" style="background:var(--err)"></span>live</span>
    </div>
    <div class="boxed">
      <div class="stream">
        {#each lines?.result.entries ?? [] as e (e.id)}
          <LogRow {e} service={false} {terms} open={F.openRows.has(e.id)} ontoggle={() => F.toggleRow(e.id)} />
        {:else}
          <div class="empty">{lines ? 'No lines stored for this service yet.' : ''}</div>
        {/each}
      </div>
    </div>
    {#if lines && lines.result.total > lines.result.entries.length}
      <span class="muted" style="font-size:12px;display:flex;gap:10px;align-items:center;flex-wrap:wrap">Newest {lines.result.entries.length.toLocaleString()} of {lines.result.total.toLocaleString()} lines.
        {#if F.limit < LINES.max}<button class="btn ghost sm" onclick={() => F.more()}>Show {Math.min(LINES.step, lines.result.total - lines.result.entries.length).toLocaleString()} more</button>{/if}
        Open in Logs for groups and download.</span>
    {/if}
  {:else}
    {#each groups as g (g.fp)}
      <div class="ecard">
        <div class="eh">
          <span class="ecount" class:w={g.level === 'WARN'}>{g.count.toLocaleString()}</span>
          <span class="ettl">{#each patternParts(g.pattern || firstLine(g.sample)) as part, i (i)}{#if part.v}<var>{part.text}</var>{:else}{part.text}{/if}{/each}</span>
          <Spark values={g.spark} warn={g.level === 'WARN'} />
        </div>
        <span class="muted emeta">{g.exc ? 'exception' : g.level === 'WARN' ? 'warning' : 'log error'} · first {ago(g.first / 1000, app.now)} · last {ago(g.last / 1000, app.now)} · {g.sources} {g.sources === 1 ? 'container' : 'containers'}</span>
        <pre class="trace" class:plain={g.level !== 'ERROR'}>{#each g.sample.split('\n') as l, i (i)}{#if i === 0}<b>{l}</b>{:else}{'\n'}<span class="fr">{l}</span>{/if}{/each}</pre>
        <div class="eacts">
          <button class="btn sm" onclick={() => { app.showLogs([s!.fullName]); app.logs.group = g.fp }}>Show all {g.count.toLocaleString()} in Logs</button>
          <button class="btn ghost sm" onclick={() => app.copy(g.sample)}><Icon name="copy" size={13} />Copy {g.exc ? 'stack trace' : 'message'}</button>
        </div>
      </div>
    {:else}
      <div class="empty">{hist ? `No errors or warnings from this service since ${hhmm(win.from)}.` : ''}</div>
    {/each}
  {/if}
</Drawer>
