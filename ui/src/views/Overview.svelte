<script lang="ts">
  import { app } from '../lib/state.svelte'
  import { api, query } from '../lib/api'
  import { ago, bytes, pct, plural } from '../lib/format'
  import { memPct, nodeDot, nodeRole, svcState } from '../lib/health'
  import type { LogResponse, ServiceRef } from '../lib/types'
  import Ring from '../components/Ring.svelte'
  import ErrSpark from '../components/ErrSpark.svelte'
  import Histogram from '../components/Histogram.svelte'
  import Icon from '../components/Icon.svelte'
  import PinnedTable from '../components/PinnedTable.svelte'

  let snap = $derived(app.snap!)
  let single = $derived(snap.nodes.length <= 1)
  let host = $derived(snap.host)
  let services = $derived(app.services.filter((s) => s.desired > 0 || s.running > 0))
  let healthy = $derived(services.filter((s) => s.running >= s.desired).length)
  let tasks = $derived(app.services.flatMap((s) => s.tasks))
  let running = $derived(tasks.filter((t) => t.state === 'running').length)
  let managers = $derived(snap.nodes.filter((n) => n.role === 'manager'))
  let ready = $derived(snap.nodes.filter((n) => n.state === 'ready').length)
  let err1 = $derived(app.errors?.lastHour ?? 0)
  let errPrev = $derived(app.errors?.prevHour ?? 0)
  let hostCpu = $derived(host.cpuPercent ?? snap.nodes[0]?.cpuPercent ?? null)
  let hostMem = $derived(memPct(host.memUsed, host.memTotal))

  // Errors and warnings of the last 6 hours, refreshed with the snapshot.
  let hist = $state<LogResponse | null>(null)
  let windowEnd = $state(Date.now())
  $effect(() => {
    const id = app.activeId
    void app.snap?.fetchedAt
    const to = Date.now()
    const from = to - 6 * 3600e3
    api<LogResponse>(`/c/${encodeURIComponent(id)}/logs?${query({ from, to, levels: 'ERROR,WARN', buckets: 48, limit: 1 })}`)
      .then((r) => {
        hist = r
        windowEnd = to
      })
      .catch(() => {})
  })
  let top = $derived(hist?.result.groups[0])

  type Attn = { sev: 'err' | 'warn'; title: string; sub: string; onclick: () => void }
  let attn = $derived.by(() => {
    const out: Attn[] = []
    for (const n of snap.nodes) {
      if (n.state !== 'ready') out.push({ sev: 'err', title: `Node <b>${esc(n.hostname)}</b> is ${esc(n.state)}`, sub: nodeRole(n), onclick: () => (app.drawer = { kind: 'node', id: n.id }) })
      else if (n.availability !== 'active') out.push({ sev: 'warn', title: `Node <b>${esc(n.hostname)}</b> is set to ${esc(n.availability)}`, sub: 'No new tasks are scheduled here', onclick: () => (app.drawer = { kind: 'node', id: n.id }) })
    }
    const lastErr = (s: ServiceRef) => s.tasks.find((t) => t.error || (t.state !== 'running' && t.message))
    for (const s of services) {
      if (s.running === 0 && s.desired > 0) {
        const t = lastErr(s)
        out.push({ sev: 'err', title: `<b>${esc(s.fullName)}</b> has nothing running`, sub: `0/${s.desired}${t ? ' · ' + esc(t.error || t.message || '') : ''}`, onclick: () => open(s) })
      } else if (s.running < s.desired) {
        out.push({ sev: 'warn', title: `<b>${esc(s.fullName)}</b> is under-replicated`, sub: `${s.running}/${s.desired} running`, onclick: () => open(s) })
      }
    }
    for (const s of services) {
      const t = s.tasks.find((t) => t.state === 'running' && t.health === 'unhealthy')
      if (t) out.push({ sev: 'warn', title: `<b>${esc(s.fullName)}</b> reports unhealthy`, sub: `replica ${esc(t.slot)}`, onclick: () => open(s) })
    }
    if (err1) {
      out.push({
        sev: 'err',
        title: `<b>${err1}</b> ${err1 === 1 ? 'error' : 'errors'} in the last hour`,
        sub: top ? `Most frequent: ${esc(top.pattern.slice(0, 110))} (${top.count}×)` : 'Open the logs to see them grouped',
        onclick: () => {
          app.logs.reset()
          app.logs.setPreset('1h')
          app.logs.levels.clear()
          app.logs.levels.add('ERROR')
          app.go('logs')
        },
      })
    }
    for (const w of snap.warnings) out.push({ sev: 'warn', title: esc(w), sub: '', onclick: () => {} })
    return out
  })

  function esc(s: string) {
    return s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]!)
  }
  function open(s: ServiceRef) {
    app.drawer = { kind: 'service', key: s.key }
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

<div class="kpis">
  {#if single}
    <div class="panel kpi">
      <span class="lbl">Host · {host.name}</span>
      <span class="val">{hostCpu === null ? '–' : hostCpu.toFixed(0) + '%'}<small> CPU</small></span>
      <span class="sub" title="CPU and memory used by containers">{bytes(host.memUsed)} memory · {host.ncpu} cores</span>
      <Ring value={hostCpu ?? 0} total={100} load />
    </div>
  {:else}
    <div class="panel kpi">
      <span class="lbl">Nodes ready</span>
      <span class="val">{ready}<small>/{snap.nodes.length}</small></span>
      <span class="sub">{plural(managers.length, 'manager')} · quorum
        {#if managers.filter((m) => m.state === 'ready').length > managers.length / 2}<span class="pill ok">healthy</span>{:else}<span class="pill err">lost</span>{/if}</span>
      <Ring value={ready} total={snap.nodes.length} />
    </div>
  {/if}
  <div class="panel kpi">
    <span class="lbl">Services healthy</span>
    <span class="val">{healthy}<small>/{services.length}</small></span>
    <span class="sub">across {plural(snap.stacks.length, 'stack')}</span>
    <Ring value={healthy} total={services.length} />
  </div>
  <div class="panel kpi">
    <span class="lbl">{snap.mode === 'swarm' ? 'Tasks running' : 'Containers running'}</span>
    <span class="val">{running}<small>/{tasks.length}</small></span>
    <span class="sub">{tasks.length - running} stopped, pending or failed</span>
    <Ring value={running} total={tasks.length} />
  </div>
  <div class="panel kpi" class:alert={err1 > 0}>
    <span class="lbl">Errors · last hour</span>
    <span class="val" style:color={err1 ? 'var(--err)' : 'inherit'}>{err1}</span>
    <span class="sub">{trend()}</span>
    <ErrSpark values={app.errors?.spark ?? []} />
  </div>
</div>

{#if app.pinnedServices.length}
  <section class="panel">
    <div class="ph"><h2><Icon name="star" size={15} fill class="star-ic" /> Pinned services</h2><span class="spacer"></span><a class="btn ghost" href="#stacks">All services</a></div>
    <PinnedTable services={app.pinnedServices} />
  </section>
{/if}

<div class="grid2">
  <section class="panel">
    <div class="ph"><h2>Needs attention</h2><span class="spacer"></span><span class="muted" style="font-size:12px">{plural(attn.length, 'item')}</span></div>
    <div class="attn">
      {#each attn as a, i (i)}
        <button type="button" onclick={a.onclick}>
          <span class="sev {a.sev}"></span>
          <span>{@html a.title}{#if a.sub}<br /><span class="muted" style="font-size:12.5px">{@html a.sub}</span>{/if}</span>
          <Icon name="chev" size={12} class="chev" />
        </button>
      {:else}
        <div class="empty">All nodes and services look healthy.</div>
      {/each}
    </div>
  </section>
  <section class="panel">
    <div class="ph"><h2>Errors &amp; warnings · 6h</h2><span class="spacer"></span><a class="btn ghost" href="#logs">Open logs</a></div>
    {#if hist}
      <Histogram buckets={hist.result.histogram} from={windowEnd - 6 * 3600e3} to={windowEnd} levels={['WARN', 'ERROR']} />
    {:else}
      <div class="center-empty" style="padding:50px"><span class="spinner"></span></div>
    {/if}
    <div class="legend" style="padding:0 14px 12px 52px">
      <span><i style="background:var(--err)"></i>Errors</span><span><i style="background:var(--warn)"></i>Warnings</span><span><i class="line" style="background:var(--exc)"></i>Exceptions</span>
    </div>
  </section>
</div>

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
  <div class="ph"><h2>Stacks</h2><span class="spacer"></span><a class="btn ghost" href="#stacks">All stacks</a></div>
  <div class="tw">
    <table>
      <thead><tr><th>Stack</th><th>Services</th><th>Status</th><th class="hide-sm">Errors 1h</th><th class="hide-sm">Last update</th></tr></thead>
      <tbody>
        {#each snap.stacks as st (st.name)}
          {@const live = st.services.filter((s) => s.desired > 0 || s.running > 0)}
          {@const bad = live.filter((s) => s.running < s.desired)}
          {@const e = stackErrors(st.name)}
          {@const upd = Math.max(0, ...st.services.map((s) => s.updatedAt ?? 0))}
          <tr class="click" onclick={() => app.go('stacks')}>
            <td><b>{st.name}</b> <span class="kind">{st.kind}</span></td>
            <td>{st.services.length}</td>
            <td>
              {#if bad.length}{#if bad.some((s) => !s.running)}<span class="pill err">failing</span>{:else}<span class="pill warn">degraded</span>{/if}{:else}<span class="pill ok">healthy</span>{/if}
            </td>
            <td class="hide-sm mono" style:color={e ? 'var(--err)' : 'var(--muted)'}>{e}</td>
            <td class="hide-sm muted">{upd ? ago(upd, app.now) : '–'}</td>
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
