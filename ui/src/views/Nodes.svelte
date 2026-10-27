<script lang="ts">
  import { app } from '../lib/state.svelte'
  import { bytes, pct, plural } from '../lib/format'
  import { memPct, nodeRole } from '../lib/health'

  let snap = $derived(app.snap!)
  let engines = $derived(new Set(snap.nodes.map((n) => n.engine)))
  let common = $derived.by(() => {
    const c = new Map<string, number>()
    for (const n of snap.nodes) c.set(n.engine, (c.get(n.engine) ?? 0) + 1)
    return [...c.entries()].sort((a, b) => b[1] - a[1])[0]?.[0]
  })
</script>

{#if snap.mode === 'standalone'}
  <div class="panel pb" style="display:flex;gap:10px;align-items:center;flex-wrap:wrap">
    <span class="pill acc">Plain Docker</span><span class="muted">This host is not part of a swarm. Containers are grouped by their Compose project.</span>
  </div>
{:else if snap.nodes.length === 1}
  <div class="panel pb" style="display:flex;gap:10px;align-items:center;flex-wrap:wrap">
    <span class="pill acc">Single-node swarm</span><span class="muted">This node is both manager and worker.</span>
  </div>
{/if}

<section class="panel">
  <div class="ph">
    <h2>{plural(snap.nodes.length, 'node')}</h2><span class="spacer"></span>
    <span class="legend"><span><span class="dot"></span>ready</span><span><span class="dot warn"></span>drain / pause</span><span><span class="dot err"></span>down</span></span>
  </div>
  <div class="tw">
    <table>
      <thead><tr><th>Node</th><th>Role</th><th>Status</th><th>Availability</th><th>CPU</th><th>Memory</th><th>Running</th><th class="hide-sm">Engine</th><th class="hide-sm">Address</th></tr></thead>
      <tbody>
        {#each snap.nodes as n (n.id)}
          {@const mem = memPct(n.memUsed, n.memTotal)}
          <tr class="click" onclick={() => (app.drawer = { kind: 'node', id: n.id })}>
            <td><b>{n.hostname}</b>{#if n.isLocal && snap.nodes.length > 1} <span class="kind">this</span>{/if}</td>
            <td>{nodeRole(n)}</td>
            <td>{#if n.state === 'ready'}<span class="pill ok">ready</span>{:else}<span class="pill err">{n.state}</span>{/if}</td>
            <td>{#if n.availability === 'active'}<span class="muted">active</span>{:else}<span class="pill warn">{n.availability}</span>{/if}</td>
            <td><div style="display:flex;gap:8px;align-items:center"><span class="bar"><i class:hot={(n.cpuPercent ?? 0) > 70} style:width="{n.cpuPercent ?? 0}%"></i></span><span class="mono">{pct(n.cpuPercent)}</span></div></td>
            <td><div style="display:flex;gap:8px;align-items:center"><span class="bar"><i class:hot={(mem ?? 0) > 75} style:width="{mem ?? 0}%"></i></span><span class="mono">{bytes(n.memUsed)}/{bytes(n.memTotal)}</span></div></td>
            <td class="mono">{n.tasksRunning}</td>
            <td class="hide-sm mono" style:color={engines.size > 1 && n.engine !== common ? 'var(--warn)' : 'var(--muted)'}>{n.engine}</td>
            <td class="hide-sm mono muted">{n.addr}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
  {#if snap.nodes.some((n) => !n.isLocal && n.cpuPercent === null)}
    <div class="pb hint">CPU and memory are only read for the node swarmdash is connected to. Docker does not offer them for other nodes without connecting to each one.</div>
  {/if}
</section>
