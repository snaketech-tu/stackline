<script lang="ts">
  import { app } from '../lib/state.svelte'
  import { fetchEvents, fetchLogs } from '../lib/api'
  import { eventLabel, isCrash } from '../lib/events'
  import { ago, bytes, firstLine, hhmm, pct, plural, tidy } from '../lib/format'
  import { memPct, nodeDot, nodeRole, stackState, svcProblem } from '../lib/health'
  import type { DockerEvent, LogResponse, ServiceRef } from '../lib/types'
  import Ring from '../components/Ring.svelte'
  import ErrSpark from '../components/ErrSpark.svelte'
  import Histogram from '../components/Histogram.svelte'
  import Icon from '../components/Icon.svelte'
  import ServiceTable from '../components/ServiceTable.svelte'

  let snap = $derived(app.snap!)
  let single = $derived(snap.nodes.length <= 1)
  let host = $derived(snap.host)
  let services = $derived(app.services.filter((s) => s.desired > 0 || s.running > 0))
  // The same rule as the Stacks page: all replicas up and none failing its health check.
  let healthy = $derived(services.filter((s) => !svcProblem(s)).length)
  let tasks = $derived(app.services.flatMap((s) => s.tasks))
  let running = $derived(tasks.filter((t) => t.state === 'running').length)
  let managers = $derived(snap.nodes.filter((n) => n.role === 'manager'))
  let ready = $derived(snap.nodes.filter((n) => n.state === 'ready').length)
  let err1 = $derived(app.errors?.lastHour ?? 0)
  let errPrev = $derived(app.errors?.prevHour ?? 0)
  let hostCpu = $derived(host.cpuPercent ?? snap.nodes[0]?.cpuPercent ?? null)
  let hostMem = $derived(memPct(host.memUsed, host.memTotal))

  // Errors and warnings of the last 6 hours (or since the oldest stored line), refreshed with the snapshot.
  let hist = $state.raw<LogResponse | null>(null)
  let win = $state({ from: Date.now() - 6 * 3600e3, to: Date.now() })
  $effect(() => {
    const id = app.activeId
    void app.snap?.fetchedAt
    const r = app.storedRange(6 * 3600e3)
    fetchLogs(id, { ...r, levels: 'ERROR,WARN', buckets: 48, limit: 1 })
      .then((x) => {
        // An answer for a connection that is no longer selected must not show up here.
        if (id !== app.activeId) return
        hist = x
        win = r
      })
      .catch(() => {})
  })
  // Crashes and failed health checks of the chart's window, marked on it and listed when recent.
  let events = $state.raw<DockerEvent[]>([])
  $effect(() => {
    const id = app.activeId
    void app.snap?.fetchedAt
    const to = Date.now()
    fetchEvents(id, { from: to - 6 * 3600e3, to: to + 60e3, limit: 2000 })
      .then((x) => {
        if (id === app.activeId) events = x.filter((e) => isCrash(e) || e.kind === 'unhealthy')
      })
      .catch(() => {})
  })
  /** Services that crashed in the last hour, newest crash first. */
  let crashed = $derived.by(() => {
    const since = app.now - 3600e3
    const by = new Map<string, DockerEvent[]>()
    for (const e of events) if (isCrash(e) && e.ts / 1000 >= since) by.set(e.service, [...(by.get(e.service) ?? []), e])
    return [...by.values()].map((es) => ({ last: es[es.length - 1], n: es.length })).sort((a, b) => b.last.ts - a.last.ts)
  })

  // The most frequent error of the chart's window, which is longer than the hour the count covers.
  let top = $derived(hist?.result.groups.find((g) => g.level === 'ERROR'))
  let topSpan = $derived(win.to - win.from > 5.9 * 3600e3 ? 'in the last 6 h' : `since ${hhmm(win.from)}`)

  // The newest line of each service that has nothing running: usually the reason it fails.
  let lastLines = $state<Record<string, string>>({})
  let failing = $derived(services.filter((s) => s.running === 0 && s.desired > 0).slice(0, 6))
  $effect(() => {
    const id = app.activeId
    void app.snap?.fetchedAt
    const to = Date.now()
    for (const s of failing) {
      fetchLogs(id, { from: to - 6 * 3600e3, to, service: s.fullName, buckets: 1, limit: 1 })
        .then((r) => {
          const e = r.result.entries[0]
          if (e && id === app.activeId) lastLines[s.fullName] = tidy(firstLine(e.text))
        })
        .catch(() => {})
    }
  })

  type Act = { label: string; run: () => void; ghost?: boolean }
  type Attn = { sev: 'err' | 'warn'; title: string; pill?: [string, string]; why: string; acts: Act[] }
  let attn = $derived.by(() => {
    const out: Attn[] = []
    for (const n of snap.nodes) {
      const details = { label: 'Details', run: () => (app.drawer = { kind: 'node', id: n.id }) }
      if (n.state !== 'ready') out.push({ sev: 'err', title: `Node <b>${esc(n.hostname)}</b> is ${esc(n.state)}`, why: nodeRole(n), acts: [details] })
      else if (n.availability !== 'active') out.push({ sev: 'warn', title: `Node <b>${esc(n.hostname)}</b> is set to ${esc(n.availability)}`, why: 'No new tasks are scheduled here', acts: [details] })
    }
    const lastErr = (s: ServiceRef) => s.tasks.find((t) => t.error || (t.state !== 'running' && t.message))
    const acts = (s: ServiceRef): Act[] => [
      { label: 'Logs', run: () => app.openService(s.key, 'logs') },
      { label: 'Details', run: () => app.openService(s.key), ghost: true },
    ]
    for (const s of services) {
      if (s.running === 0 && s.desired > 0) {
        const t = lastErr(s)
        const fails = s.tasks.reduce((a, t) => a + t.failures, 0)
        const why = [t?.exitCode ? `exit ${t.exitCode}` : '', fails ? `${fails} earlier ${fails === 1 ? 'task' : 'tasks'} failed` : '', lastLines[s.fullName] ?? t?.error ?? t?.message ?? '']
        out.push({ sev: 'err', title: `<b>${esc(s.fullName)}</b> has nothing running`, pill: ['err', `0/${s.desired}`], why: why.filter(Boolean).join(' · '), acts: acts(s) })
      } else if (s.running < s.desired) {
        out.push({ sev: 'warn', title: `<b>${esc(s.fullName)}</b> is under-replicated`, pill: ['warn', `${s.running}/${s.desired}`], why: lastErr(s)?.error ?? `${s.desired - s.running} replica${s.desired - s.running === 1 ? '' : 's'} missing`, acts: acts(s) })
      }
    }
    for (const { last, n } of crashed) {
      const s = app.services.find((x) => x.fullName === last.service)
      out.push({
        sev: n >= 3 ? 'err' : 'warn',
        title: `<b>${esc(last.service)}</b> ${n === 1 ? 'crashed once' : `crashed ${n} times`} in the last hour`,
        why: `${n === 1 ? '' : 'last: '}replica ${last.replica} ${eventLabel(last)} at ${hhmm(last.ts / 1000)}${last.detail && last.kind !== 'failed' ? ' · ' + last.detail : ''}`,
        acts: [
          { label: 'Lines before', run: () => app.showEvent(last) },
          ...(s ? [{ label: 'Details', run: () => app.openService(s.key), ghost: true }] : []),
        ],
      })
    }
    for (const s of services) {
      const t = s.tasks.find((t) => t.state === 'running' && t.health === 'unhealthy')
      if (t) out.push({ sev: 'warn', title: `<b>${esc(s.fullName)}</b> reports unhealthy`, why: `replica ${t.slot} · ${t.message ?? ''}`, acts: acts(s) })
    }
    for (const g of app.errors?.newGroups ?? []) {
      out.push({
        sev: 'warn',
        title: `<b>New error</b> in ${esc(g.service)}`,
        pill: ['warn', `new · ${g.count}×`],
        why: g.pattern || firstLine(g.sample),
        acts: [{ label: 'Show', run: () => app.showGroup(g.fp, [g.service]) }],
      })
    }
    if (err1) {
      out.push({
        sev: 'err',
        title: `<b>${err1}</b> ${err1 === 1 ? 'error' : 'errors'} in the last hour`,
        why: top ? `Most frequent ${topSpan}: ${(top.pattern || firstLine(top.sample)).slice(0, 140)} (${top.count}×${top.services.length ? ', ' + top.services.join(', ') : ''})` : 'Open the logs to see them grouped',
        acts: [{ label: 'Errors', run: errorsOnly }, ...(top ? [{ label: 'Top group', run: () => app.showGroup(top!.fp, top!.services), ghost: true }] : [])],
      })
    }
    for (const w of snap.warnings) out.push({ sev: 'warn', title: esc(w), why: '', acts: [] })
    return out
  })

  function errorsOnly() {
    app.showLogs([], ['ERROR'])
    app.logs.setPreset('1h')
  }

  function esc(s: string) {
    return s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]!)
  }
  function openStack(name: string) {
    app.stackQ = name
    app.stackFilter = 'all'
    app.go('stacks')
  }
  function stackErrors(name: string) {
    const st = snap.stacks.find((s) => s.name === name)
    return st ? st.services.reduce((a, s) => a + (app.errors?.byService[s.fullName] ?? 0), 0) : 0
  }
  function trend(): string {
    if (!errPrev) return err1 ? 'none the hour before' : 'quiet'
    if (err1 >= 2 * errPrev) return `▲ ${(err1 / errPrev).toFixed(1)}× prev. hour`
    const d = Math.round(((err1 - errPrev) / errPrev) * 100)
    return `${d >= 0 ? '▲' : '▼'} ${Math.abs(d)}% vs prev. hour`
  }
</script>

{#if app.activeAlert}
  {@const a = app.activeAlert}
  <div class="err-banner alert-banner" role="alert">
    <Icon name="bell" size={16} />
    <span><b>{plural(a.count, 'error')} in the last {a.windowMins} minutes on {a.name}</b> · alert at {a.threshold}{#if a.since}, since {hhmm(a.since)}{/if}
      {#if a.top}<span class="why mono" title={a.top}>Most frequent: {a.top}{#if a.topService} ({a.topService}){/if}</span>{/if}</span>
    <span class="spacer"></span>
    <button class="btn sm" onclick={() => app.openAlert(a)}>Show errors</button>
  </div>
{/if}

<div class="kpis">
  {#if single}
    <button class="panel kpi" onclick={() => snap.nodes[0] && (app.drawer = { kind: 'node', id: snap.nodes[0].id })}>
      <span class="lbl">Host · {host.name}</span>
      <span class="val">{hostCpu === null ? '–' : hostCpu.toFixed(0) + '%'}<small> CPU</small></span>
      <span class="sub" title="CPU and memory used by containers">{bytes(host.memUsed)} memory · {host.ncpu} cores</span>
      <Ring value={hostCpu ?? 0} total={100} load />
      <span class="go">Host details →</span>
    </button>
  {:else}
    <button class="panel kpi" onclick={() => app.go('nodes')}>
      <span class="lbl">Nodes ready</span>
      <span class="val">{ready}<small>/{snap.nodes.length}</small></span>
      <span class="sub">{plural(managers.length, 'manager')} · quorum
        {#if managers.filter((m) => m.state === 'ready').length > managers.length / 2}<span class="pill ok">healthy</span>{:else}<span class="pill err">lost</span>{/if}</span>
      <Ring value={ready} total={snap.nodes.length} />
      <span class="go">All nodes →</span>
    </button>
  {/if}
  <button class="panel kpi" onclick={() => { app.stackQ = ''; app.stackFilter = healthy < services.length ? 'problems' : 'all'; app.go('stacks') }}>
    <span class="lbl">Services healthy</span>
    <span class="val">{healthy}<small>/{services.length}</small></span>
    <span class="sub">{#if healthy < services.length}{services.length - healthy} not healthy · {/if}across {plural(snap.stacks.length, 'stack')}</span>
    <Ring value={healthy} total={services.length} />
    <span class="go">{healthy < services.length ? 'Show problems →' : 'All services →'}</span>
  </button>
  <button class="panel kpi" onclick={() => { app.stackQ = ''; app.stackFilter = 'all'; app.go('stacks') }}>
    <span class="lbl">{snap.mode === 'swarm' ? 'Tasks running' : 'Containers running'}</span>
    <span class="val">{running}<small>/{tasks.length}</small></span>
    <span class="sub">{tasks.length - running} stopped, pending or failed</span>
    <Ring value={running} total={tasks.length} />
    <span class="go">All services →</span>
  </button>
  <button class="panel kpi" class:alert={err1 > 0} onclick={errorsOnly}>
    <span class="lbl">Errors · last hour</span>
    <span class="val" style:color={err1 ? 'var(--err)' : 'inherit'}>{err1}</span>
    <span class="sub">{trend()}</span>
    <ErrSpark values={app.errors?.spark ?? []} />
    <span class="go">Show errors →</span>
  </button>
</div>

<div class="grid2" style="align-items:start">
  <section class="panel">
    <div class="ph"><h2>Needs attention</h2><span class="spacer"></span><span class="muted" style="font-size:12px">{plural(attn.length, 'item')}</span></div>
    <div class="attn">
      {#each attn as a, i (i)}
        <div class="ai">
          <span class="sev {a.sev}"></span>
          <span style="min-width:0">{@html a.title}{#if a.pill}<span class="pill {a.pill[0]}" style="margin-left:6px">{a.pill[1]}</span>{/if}{#if a.why}<span class="why" title={a.why}>{a.why}</span>{/if}</span>
          <span class="acts2">{#each a.acts as x (x.label)}<button class="btn sm" class:ghost={x.ghost} onclick={x.run}>{x.label}</button>{/each}</span>
        </div>
      {:else}
        <div class="empty">All nodes and services look healthy.</div>
      {/each}
    </div>
  </section>
  <section class="panel">
    <div class="ph"><h2>Errors &amp; warnings · {win.to - win.from > 5.9 * 3600e3 ? '6h' : `since ${hhmm(win.from)}`}</h2><span class="pill hide-sm" title="Only logs of the selected connection are shown here">{host.name}</span><span class="spacer"></span><button class="btn ghost sm" onclick={() => app.showLogs([], ['ERROR', 'WARN'])}>Open in Logs</button></div>
    {#if hist}
      <Histogram buckets={hist.result.histogram} from={win.from} to={win.to} levels={['WARN', 'ERROR']} marks={events} onrange={(a, z) => { app.showLogs([], ['ERROR', 'WARN']); app.logs.zoom(a, z) }} />
    {:else}
      <div class="center-empty" style="padding:50px"><span class="spinner"></span></div>
    {/if}
    <div class="legend" style="padding:0 14px 12px 52px">
      <span><i style="background:var(--err)"></i>Errors</span><span><i style="background:var(--warn)"></i>Warnings</span><span><i class="line" style="background:var(--exc)"></i>Exceptions</span>
      {#if events.length}<span><i class="mark" style="background:var(--err)"></i>Crashes</span>{/if}
    </div>
  </section>
</div>

{#if app.pinnedServices.length}
  <section class="panel">
    <div class="ph"><h2><Icon name="star" size={15} fill class="star-ic" /> Pinned services</h2><span class="spacer"></span><button class="btn ghost sm" onclick={() => { app.stackQ = ''; app.stackFilter = 'pinned'; app.go('stacks') }}>All pinned</button></div>
    <ServiceTable services={app.pinnedServices} stackCol full={false} />
  </section>
{/if}

{#if single}
  <section class="panel">
    <div class="ph">
      <h2>Host</h2>
      <span class="mono muted hide-sm" style="font-size:12px">{host.name} · {host.os} · Docker {host.dockerVersion}</span>
      <span class="spacer"></span>
      {#if snap.nodes[0]}<button class="btn ghost" onclick={() => (app.drawer = { kind: 'node', id: snap.nodes[0].id })}>Details</button>{/if}
    </div>
    <div class="pb meters">
      {@render meter(`Container CPU · ${host.ncpu} cores`, hostCpu, hostCpu === null ? '–' : hostCpu.toFixed(1) + '%')}
      {@render meter('Container memory', hostMem, `${bytes(host.memUsed)} / ${bytes(host.memTotal)}`)}
      {@render meter('Containers running', host.containersTotal ? (host.containersRunning / host.containersTotal) * 100 : 0, `${host.containersRunning} / ${host.containersTotal}`)}
      {@render meter('Images', null, String(host.images))}
    </div>
  </section>
{:else}
  <section class="panel">
    <div class="ph"><h2>Nodes</h2><span class="spacer"></span><a class="btn ghost" href="#nodes">All nodes</a></div>
    <div class="pb nodes-cards">
      {#each snap.nodes as n (n.id)}
        {@const mem = memPct(n.memUsed, n.memTotal)}
        <button type="button" class="ncard" onclick={() => (app.drawer = { kind: 'node', id: n.id })}>
          <div class="h"><span class="dot {nodeDot(n)}"></span>{n.hostname}<span class="spacer"></span><span class="muted" style="font-weight:400;font-size:11.5px">{n.role === 'manager' ? (n.leader ? 'leader' : 'mgr') : 'wkr'}</span></div>
          <div class="r"><span>CPU</span><span class="bar"><i class:hot={(n.cpuPercent ?? 0) > 70} style:width="{n.cpuPercent ?? 0}%"></i></span><span class="mono">{pct(n.cpuPercent)}</span></div>
          <div class="r"><span>Mem</span><span class="bar"><i class:hot={(mem ?? 0) > 75} style:width="{mem ?? 0}%"></i></span><span class="mono">{pct(mem)}</span></div>
        </button>
      {/each}
    </div>
  </section>
{/if}

<section class="panel">
  <div class="ph"><h2>Stacks</h2><span class="spacer"></span><a class="btn ghost sm" href="#stacks" onclick={() => { app.stackQ = ''; app.stackFilter = 'all' }}>All stacks</a></div>
  <div class="tw">
    <table>
      <thead><tr><th>Stack</th><th>Services</th><th>Status</th><th class="hide-sm">Errors 1h</th><th class="hide-sm">Last update</th><th class="actcell"><span class="sr">Actions</span></th></tr></thead>
      <tbody>
        {#each snap.stacks as st (st.name)}
          {@const state = stackState(st.services)}
          {@const e = stackErrors(st.name)}
          {@const upd = Math.max(0, ...st.services.map((s) => s.updatedAt ?? 0))}
          <tr class="click" onclick={() => openStack(st.name)}>
            <td><b>{st.name}</b> <span class="kind">{st.kind}</span></td>
            <td>{st.services.length}</td>
            <td>
              <span class="pill {state.cls}">{state.label}</span>{#if state.stopped}<span class="muted stopped">{state.stopped} stopped</span>{/if}
            </td>
            <td class="hide-sm mono" style:color={e ? 'var(--err)' : 'var(--muted)'}>{e}</td>
            <td class="hide-sm muted">{upd ? ago(upd, app.now) : '–'}</td>
            <td class="actcell"><span class="acts"><button class="btn ghost sm" onclick={(ev) => { ev.stopPropagation(); app.showLogs([`stack:${st.name}`]) }}><Icon name="logs" size={14} />Logs</button></span></td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
</section>

{#snippet meter(label: string, value: number | null, text: string)}
  <div class="meter">
    <span class="muted lbl">{label}</span>
    <div class="m">
      {#if value !== null}<span class="bar"><i class:hot={value > 75} style:width="{Math.min(100, value)}%"></i></span>{/if}
      <span class="mono">{text}</span>
    </div>
  </div>
{/snippet}
