<script lang="ts">
  // Every task (swarm) or container (Compose, lone containers): where it runs,
  // its state, exit code and restarts. Narrowed to a service, stack or node
  // when drilled into one.
  import { app } from '../lib/state.svelte'
  import { ago, bytes, pct } from '../lib/format'
  import { taskPill } from '../lib/health'
  import type { ServiceRef, Task } from '../lib/types'
  import ResourceList, { type Act, type Col, type Tab } from '../components/ResourceList.svelte'

  type Row = { s: ServiceRef; t: Task; key: string; name: string; pill: [string, string] }

  const L = app.lists.tasks
  let byService = $derived(app.params.get('service'))
  let byStack = $derived(app.params.get('stack'))
  let byNode = $derived(app.params.get('node'))
  let stack = $derived(byStack || (byService || byNode ? '' : app.scope))
  let swarm = $derived(app.snap?.mode === 'swarm')

  let items = $derived<Row[]>(
    app.services
      .filter((s) => (!byService || s.key === byService) && (!stack || s.stack === stack))
      .flatMap((s) =>
        s.tasks
          .filter((t) => !byNode || t.node === byNode)
          .map((t) => ({ s, t, key: `${s.key}#${t.id}`, name: `${s.name}.${t.slot}`, pill: taskPill(t) })),
      ),
  )
  let scope = $derived(byService ? `of service ${byService}` : byNode ? `on node ${byNode}` : byStack ? `of stack ${byStack}` : undefined)

  const cols: Col<Row>[] = [
    { id: 'name', label: 'Name', sort: (r) => r.name },
    { id: 'stack', label: 'Stack', sort: (r) => r.s.stack },
    { id: 'node', label: 'Node', sort: (r) => r.t.node },
    { id: 'state', label: 'State', sort: (r) => r.pill[1] },
    { id: 'exit', label: 'Exit', sort: (r) => r.t.exitCode, cls: 'num' },
    { id: 'failures', label: 'Failed before', sort: (r) => r.t.failures, cls: 'num', title: 'How often this slot failed before (swarm), or how often Docker restarted it' },
    { id: 'cpu', label: 'CPU', sort: (r) => r.t.cpuPercent, cls: 'num hide-sm' },
    { id: 'mem', label: 'Memory', sort: (r) => r.t.memUsed, cls: 'num hide-sm' },
    { id: 'since', label: 'Since', sort: (r) => -(r.t.since ?? 0), cls: 'hide-sm' },
    { id: 'message', label: 'Message', cls: 'hide-sm' },
  ]
  const tabs: Tab<Row>[] = [
    { id: 'all', label: 'All', match: () => true },
    { id: 'attention', label: 'Needs attention', match: (r) => r.pill[0] === 'err' || r.pill[0] === 'warn' },
    { id: 'running', label: 'Running', match: (r) => r.t.state === 'running' },
  ]
  const actions: Act<Row>[] = [
    { key: 'l', label: 'Logs', icon: 'logs', run: (r) => app.dock.open(r.s.fullName, `${r.s.fullName} #${r.t.slot}`, `replica:${r.t.slot}`) },
    { key: 'n', label: 'Node', run: (r) => app.drill('tasks', { node: r.t.node }), when: () => !byNode },
    { key: 'o', label: 'Logs page', run: (r) => { app.showLogs([r.s.fullName]); app.logs.q = `replica:${r.t.slot}` } },
  ]
  const match = (r: Row, q: string) =>
    r.name.toLowerCase().includes(q) || r.s.stack.toLowerCase().includes(q) || r.t.node.toLowerCase().includes(q) || r.s.image.toLowerCase().includes(q) || r.t.state.includes(q)
  let inspecting = $derived(app.drawer?.kind === 'service' ? app.drawer.key : null)
</script>

<ResourceList list={L} {items} key={(r) => r.key} label={(r) => `${r.s.fullName}.${r.t.slot}`} {cols} {tabs} {match} {actions}
  title={swarm ? 'Tasks' : 'Containers'} noun={swarm ? ['task', 'tasks'] : ['container', 'containers']}
  inspecting={inspecting ? items.find((r) => r.s.key === inspecting && r.key === L.cursor)?.key ?? null : null}
  {scope} onscopeclear={() => app.drill('tasks')}
  onopen={(r) => app.openService(r.s.key, 'overview')} openLabel="Service"
  oninspect={(r) => app.openService(r.s.key, app.drawer?.kind === 'service' ? app.drawer.tab : 'overview')}
  empty="Nothing is scheduled.">
  {#snippet row(r)}
    <td class="mono"><b>{r.name}</b></td>
    <td class="muted">{r.s.stack}</td>
    <td class="mono muted">{r.t.node}</td>
    <td><span class="pill {r.pill[0]}">{r.pill[1]}</span>{#if r.t.health && r.t.health !== 'unhealthy' && r.t.state === 'running'} <span class="muted" style="font-size:11.5px">{r.t.health}</span>{/if}</td>
    <td class="num mono" style:color={r.t.exitCode ? 'var(--err)' : 'var(--muted)'}>{r.t.exitCode ?? '–'}</td>
    <td class="num mono" style:color={r.t.failures ? 'var(--warn)' : 'var(--muted)'}>{r.t.failures}</td>
    <td class="num hide-sm mono">{pct(r.t.cpuPercent, 1)}</td>
    <td class="num hide-sm mono">{bytes(r.t.memUsed)}</td>
    <td class="hide-sm muted">{r.t.since ? ago(r.t.since, app.now) : '–'}</td>
    <td class="hide-sm muted clip" title={r.t.error || r.t.message || ''}>{r.t.error || (r.t.state !== 'running' ? r.t.message : '') || ''}</td>
  {/snippet}
</ResourceList>
