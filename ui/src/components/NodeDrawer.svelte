<script lang="ts">
  import { app } from '../lib/state.svelte'
  import { bytes, pct } from '../lib/format'
  import { memPct, nodeRole, taskPill } from '../lib/health'
  import Drawer from './Drawer.svelte'
  import Icon from './Icon.svelte'

  let { id }: { id: string } = $props()
  let n = $derived(app.snap?.nodes.find((x) => x.id === id))
  let tasks = $derived(
    n ? app.services.flatMap((s) => s.tasks.filter((t) => t.node === n!.hostname).map((t) => ({ s, t }))) : [],
  )

  function nodeLogs() {
    if (!n) return
    app.showLogs()
    app.logs.q = `node:${n.hostname}`
  }
</script>

<Drawer title={n?.hostname ?? 'Node'}>
  {#if !n}
    <div class="empty">This node is gone.</div>
  {:else}
    <div style="display:flex;gap:8px;flex-wrap:wrap;align-items:center">
      {#if n.state === 'ready'}<span class="pill ok">ready</span>{:else}<span class="pill err">{n.state}</span>{/if}
      <span class="pill">{nodeRole(n)}</span>
      {#if n.availability !== 'active'}<span class="pill warn">{n.availability}</span>{/if}
      <span class="spacer"></span>
      {#if app.snap && app.snap.nodes.length > 1}<button class="btn" onclick={nodeLogs}>Logs from this node</button>{/if}
    </div>
    <dl class="kv">
      <dt>Address</dt><dd>{n.addr || '–'}</dd>
      <dt>OS</dt><dd>{n.os}</dd>
      <dt>Engine</dt><dd>Docker {n.engine}</dd>
      <dt>Resources</dt><dd>{n.ncpu} CPUs · {bytes(n.memTotal)}</dd>
      <dt>Usage</dt><dd>CPU {pct(n.cpuPercent)} · memory {pct(memPct(n.memUsed, n.memTotal))}</dd>
      {#if n.reachability}<dt>Manager</dt><dd>{n.reachability}</dd>{/if}
    </dl>
    <div>
      <h4>Labels</h4>
      <div class="chips">
        {#each Object.entries(n.labels) as [k, v] (k)}<span class="chip">{k}={v}</span>{:else}<span class="muted">No labels</span>{/each}
      </div>
    </div>
    <div>
      <h4>{app.snap?.mode === 'swarm' ? 'Tasks' : 'Containers'} on this node ({tasks.length})</h4>
      <div class="tw boxed">
        <table>
          <thead><tr><th>Service</th><th>State</th><th class="actcell"><span class="sr">Logs</span></th></tr></thead>
          <tbody>
            {#each tasks as { s, t } (t.id)}
              {@const [pc, label] = taskPill(t)}
              <tr class="click" onclick={() => app.openService(s.key)}><td class="mono">{s.fullName}.{t.slot}</td><td><span class="pill {pc}">{label}</span></td><td class="actcell"><span class="acts"><button class="ib" title="Logs of {s.fullName}" aria-label="Logs of {s.fullName}" onclick={(ev) => { ev.stopPropagation(); app.showLogs([s.fullName]) }}><Icon name="logs" size={14} /></button></span></td></tr>
            {:else}
              <tr><td colspan="3" class="muted">Nothing scheduled here.</td></tr>
            {/each}
          </tbody>
        </table>
      </div>
    </div>
  {/if}
</Drawer>
