<script lang="ts">
  // The stacks (swarm stacks, Compose projects, lone containers) of the
  // connection. Enter shows the services of one.
  import { app } from '../lib/state.svelte'
  import { ago, bytes, pct } from '../lib/format'
  import { stackState } from '../lib/health'
  import type { Stack } from '../lib/types'
  import ResourceList, { type Act, type Col, type Tab } from '../components/ResourceList.svelte'
  import CollectSwitch from '../components/CollectSwitch.svelte'

  type Row = Stack & { state: ReturnType<typeof stackState>; errors: number; running: number; desired: number; cpu: number | null; mem: number | null; updated: number }

  const L = app.lists.stacks
  const sum = (xs: (number | null)[]) => (xs.some((x) => x !== null) ? xs.reduce<number>((a, x) => a + (x ?? 0), 0) : null)
  let items = $derived<Row[]>(
    (app.snap?.stacks ?? []).map((st) => ({
      ...st,
      state: stackState(st.services),
      errors: st.services.reduce((a, s) => a + (app.errors?.byService[s.fullName] ?? 0), 0),
      running: st.services.reduce((a, s) => a + s.running, 0),
      desired: st.services.reduce((a, s) => a + s.desired, 0),
      cpu: sum(st.services.map((s) => s.cpuPercent)),
      mem: sum(st.services.map((s) => s.memUsed)),
      updated: Math.max(0, ...st.services.map((s) => s.updatedAt ?? 0)),
    })),
  )

  const cols: Col<Row>[] = [
    { id: 'name', label: 'Stack', sort: (r) => r.name },
    { id: 'kind', label: 'Kind', sort: (r) => r.kind },
    { id: 'status', label: 'Status', sort: (r) => r.state.label },
    { id: 'services', label: 'Services', sort: (r) => r.services.length, cls: 'num' },
    { id: 'replicas', label: 'Running', sort: (r) => r.running, cls: 'num' },
    { id: 'cpu', label: 'CPU', sort: (r) => r.cpu, cls: 'num hide-sm' },
    { id: 'mem', label: 'Memory', sort: (r) => r.mem, cls: 'num hide-sm' },
    { id: 'errors', label: 'Errors 1h', sort: (r) => r.errors, cls: 'num' },
    { id: 'collect', label: 'Collect', cls: 'collectcell', title: 'Collect the logs of every service of the stack' },
    { id: 'updated', label: 'Last update', sort: (r) => r.updated, cls: 'hide-sm' },
  ]
  const tabs: Tab<Row>[] = [
    { id: 'all', label: 'All', match: () => true },
    { id: 'attention', label: 'Needs attention', match: (r) => r.state.cls === 'err' || r.state.cls === 'warn' },
  ]
  const actions: Act<Row>[] = [
    { key: 'l', label: 'Logs', icon: 'logs', run: (r) => app.dock.open(`stack:${r.name}`, `stack ${r.name}`) },
    { key: 't', label: 'Tasks', run: (r) => app.drill('tasks', { stack: r.name }) },
    { key: 's', label: 'Narrow to it', run: (r) => (app.scope = app.scope === r.name ? '' : r.name) },
    { key: 'o', label: 'Logs page', run: (r) => app.showLogs([`stack:${r.name}`]) },
  ]
  const match = (r: Row, q: string) => r.name.toLowerCase().includes(q) || r.services.some((s) => s.name.toLowerCase().includes(q) || s.image.toLowerCase().includes(q))
</script>

<ResourceList list={L} items={items} key={(r) => r.name} label={(r) => r.name} {cols} {tabs} {match} {actions}
  title="Stacks" noun={['stack', 'stacks']} onopen={(r) => app.drill('services', { stack: r.name })} openLabel="Services"
  empty="No stacks or containers on this host.">
  {#snippet row(r)}
    <td><b>{r.name}</b>{#if app.scope === r.name} <span class="kind" title="The lists are narrowed to this stack">picked</span>{/if}</td>
    <td><span class="kind">{r.kind}</span></td>
    <td><span class="pill {r.state.cls}">{r.state.label}</span>{#if r.state.stopped}<span class="muted stopped">{r.state.stopped} stopped</span>{/if}</td>
    <td class="num mono">{r.services.length}</td>
    <td class="num mono">{r.running}/{r.desired}</td>
    <td class="num hide-sm mono">{pct(r.cpu, 1)}</td>
    <td class="num hide-sm mono">{bytes(r.mem)}</td>
    <td class="num mono" style:color={r.errors ? 'var(--err)' : 'var(--muted)'}>{r.errors}</td>
    <td class="collectcell"><CollectSwitch names={r.services.map((s) => s.fullName)} label={r.name} /></td>
    <td class="hide-sm muted">{r.updated ? ago(r.updated, app.now) : '–'}</td>
  {/snippet}
</ResourceList>
