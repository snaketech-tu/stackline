<script lang="ts">
  import { app } from '../lib/state.svelte'
  import { bytes, pct } from '../lib/format'
  import { memPct, nodeRole } from '../lib/health'
  import type { Node } from '../lib/types'
  import ResourceList, { type Act, type Col, type Tab } from '../components/ResourceList.svelte'

  const L = app.lists.nodes
  let snap = $derived(app.snap!)
  let common = $derived.by(() => {
    const c = new Map<string, number>()
    for (const n of snap.nodes) c.set(n.engine, (c.get(n.engine) ?? 0) + 1)
    return [...c.entries()].sort((a, b) => b[1] - a[1])[0]?.[0]
  })
  let engines = $derived(new Set(snap.nodes.map((n) => n.engine)).size)

  const cols: Col<Node>[] = [
    { id: 'name', label: 'Node', sort: (n) => n.hostname },
    { id: 'role', label: 'Role', sort: (n) => nodeRole(n) },
    { id: 'state', label: 'Status', sort: (n) => n.state },
    { id: 'avail', label: 'Availability', sort: (n) => n.availability },
    { id: 'cpu', label: 'CPU', sort: (n) => n.cpuPercent },
    { id: 'mem', label: 'Memory', sort: (n) => memPct(n.memUsed, n.memTotal) },
    { id: 'tasks', label: 'Running', sort: (n) => n.tasksRunning, cls: 'num' },
    { id: 'engine', label: 'Engine', sort: (n) => n.engine, cls: 'hide-sm' },
    { id: 'addr', label: 'Address', sort: (n) => n.addr, cls: 'hide-sm' },
  ]
  const tabs: Tab<Node>[] = [
    { id: 'all', label: 'All', match: () => true },
    { id: 'attention', label: 'Needs attention', match: (n) => n.state !== 'ready' || n.availability !== 'active' },
  ]
  const actions: Act<Node>[] = [
    { key: 'l', label: 'Logs', icon: 'logs', run: (n) => app.dock.open('', `node ${n.hostname}`, `node:${n.hostname}`) },
  ]
  const match = (n: Node, q: string) => n.hostname.toLowerCase().includes(q) || n.addr.includes(q) || n.role.includes(q)
  let inspecting = $derived(app.drawer?.kind === 'node' ? app.drawer.id : null)
</script>

<ResourceList list={L} items={snap.nodes} key={(n) => n.id} label={(n) => n.hostname} {cols} {tabs} {match} {actions}
  title="Nodes" noun={['node', 'nodes']} {inspecting}
  onopen={(n) => app.drill('tasks', { node: n.hostname })} openLabel={snap.mode === 'swarm' ? 'Tasks' : 'Containers'}
  oninspect={(n) => (app.drawer = { kind: 'node', id: n.id })}>
  {#snippet before()}
    {#if snap.mode === 'standalone'}
      <div class="hint"><span class="pill acc">Plain Docker</span> This host is not part of a swarm. Containers are grouped by their Compose project.</div>
    {:else if snap.nodes.length === 1}
      <div class="hint"><span class="pill acc">Single-node swarm</span> This node is both manager and worker.</div>
    {/if}
    {#if snap.nodes.some((n) => !n.isLocal && n.cpuPercent === null)}
      <div class="hint">CPU and memory are only read for the node Stackline is connected to. Docker does not offer them for other nodes without connecting to each one.</div>
    {/if}
  {/snippet}
  {#snippet row(n)}
    {@const mem = memPct(n.memUsed, n.memTotal)}
    <td><b>{n.hostname}</b>{#if n.isLocal && snap.nodes.length > 1} <span class="kind">this</span>{/if}</td>
    <td>{nodeRole(n)}</td>
    <td>{#if n.state === 'ready'}<span class="pill ok">ready</span>{:else}<span class="pill err">{n.state}</span>{/if}</td>
    <td>{#if n.availability === 'active'}<span class="muted">active</span>{:else}<span class="pill warn">{n.availability}</span>{/if}</td>
    <td><div class="meterc"><span class="bar"><i class:hot={(n.cpuPercent ?? 0) > 70} style:width="{n.cpuPercent ?? 0}%"></i></span><span class="mono">{pct(n.cpuPercent)}</span></div></td>
    <td><div class="meterc"><span class="bar"><i class:hot={(mem ?? 0) > 75} style:width="{mem ?? 0}%"></i></span><span class="mono">{bytes(n.memUsed)}/{bytes(n.memTotal)}</span></div></td>
    <td class="num mono">{n.tasksRunning}</td>
    <td class="hide-sm mono" style:color={engines > 1 && n.engine !== common ? 'var(--warn)' : 'var(--muted)'}>{n.engine}</td>
    <td class="hide-sm mono muted">{n.addr}</td>
  {/snippet}
</ResourceList>
