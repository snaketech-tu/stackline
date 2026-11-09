<script lang="ts">
  // Every service of the connection in one list, narrowed by the stack picker
  // or a stack drilled into. Enter shows its tasks, a click its inspector.
  import { app, pinKey } from '../lib/state.svelte'
  import { ago, bytes, pct } from '../lib/format'
  import { svcProblem, svcState } from '../lib/health'
  import type { ServiceRef } from '../lib/types'
  import ResourceList, { type Act, type Col, type Tab } from '../components/ResourceList.svelte'
  import PinButton from '../components/PinButton.svelte'
  import CollectSwitch from '../components/CollectSwitch.svelte'
  import Icon from '../components/Icon.svelte'

  const L = app.lists.services
  /** A stack drilled into; otherwise the stack picker narrows the list. */
  let drilled = $derived(app.params.get('stack'))
  let stack = $derived(drilled || app.scope)
  let items = $derived(stack ? app.services.filter((s) => s.stack === stack) : app.services)
  const errs = (s: ServiceRef) => app.errors?.byService[s.fullName] ?? 0

  const cols: Col<ServiceRef>[] = [
    { id: 'pin', label: '', cls: 'pincell' },
    { id: 'name', label: 'Service', sort: (s) => s.name },
    { id: 'stack', label: 'Stack', sort: (s) => s.stack },
    { id: 'replicas', label: 'Replicas', sort: (s) => (s.desired ? s.running / s.desired : 2) },
    { id: 'status', label: 'Status', sort: (s) => svcState(s)[1] },
    { id: 'image', label: 'Image', sort: (s) => s.image, cls: 'hide-sm' },
    { id: 'ports', label: 'Ports', cls: 'hide-sm' },
    { id: 'cpu', label: 'CPU', sort: (s) => s.cpuPercent, cls: 'num hide-sm' },
    { id: 'mem', label: 'Memory', sort: (s) => s.memUsed, cls: 'num hide-sm' },
    { id: 'errors', label: 'Errors 1h', sort: (s) => errs(s), cls: 'num' },
    { id: 'collect', label: 'Collect', cls: 'collectcell', title: 'Whether Stackline collects and stores the logs of the service' },
    { id: 'updated', label: 'Updated', sort: (s) => s.updatedAt, cls: 'hide-sm' },
  ]
  const tabs: Tab<ServiceRef>[] = [
    { id: 'all', label: 'All', match: () => true },
    { id: 'attention', label: 'Needs attention', match: (s) => svcProblem(s) },
    { id: 'pinned', label: 'Pinned', match: (s) => app.isPinned(s.key) },
  ]
  const actions: Act<ServiceRef>[] = [
    { key: 'l', label: 'Logs', icon: 'logs', run: (s) => app.dock.open(s.fullName, s.fullName) },
    { key: 'e', label: 'Errors', run: (s) => app.openService(s.key, 'errors') },
    { key: 'o', label: 'Logs page', run: (s) => app.showLogs([s.fullName]) },
    { key: 'p', label: 'Pin', run: (s) => app.togglePin(s.key), hidden: true },
  ]
  const match = (s: ServiceRef, q: string) =>
    s.name.toLowerCase().includes(q) || s.image.toLowerCase().includes(q) || s.stack.toLowerCase().includes(q) || s.tasks.some((t) => t.node.toLowerCase().includes(q))

  // Pinned services first, then by stack and name, unless a column is sorted.
  let ordered = $derived(
    [...items].sort((a, b) => Number(app.isPinned(b.key)) - Number(app.isPinned(a.key)) || a.stack.localeCompare(b.stack) || a.name.localeCompare(b.name)),
  )
  let inspecting = $derived(app.drawer?.kind === 'service' ? app.drawer.key : null)
  let collected = $derived(app.services.filter((s) => app.collects(s.fullName)).length)
</script>

<ResourceList list={L} items={ordered} key={(s) => s.key} label={(s) => s.fullName} {cols} {tabs} {match} {actions}
  title="Services" noun={['service', 'services']} {inspecting}
  scope={drilled ? `of stack ${drilled}` : undefined} onscopeclear={() => app.drill('services')}
  onopen={(s) => app.drill('tasks', { service: pinKey(s) })} openLabel="Tasks"
  oninspect={(s) => app.openService(s.key, app.drawer?.kind === 'service' ? app.drawer.tab : 'overview')}
  empty={stack ? `No services in the stack ${stack}.` : 'No services or containers on this host.'}
  rowClass={(s) => (app.isPinned(s.key) ? 'pinned' : '')}>
  {#snippet toolbar()}
    <div class="seg" role="group" aria-label="Collect logs of" title="Which services Stackline reads and stores logs for">
      <button aria-pressed={!app.collect.only} onclick={() => app.setCollectOnly(false)}>Collect all</button>
      <button aria-pressed={app.collect.only} onclick={() => app.setCollectOnly(true)}>Only chosen<span class="n">{collected}</span></button>
    </div>
  {/snippet}
  {#snippet before()}
    {#if app.collect.only && !collected && app.services.length}
      <div class="hint"><Icon name="logs" size={14} /> No logs are collected right now. Switch on the services you want to follow in the Collect column.</div>
    {:else if !app.pinnedServices.length && L.tab === 'all' && !L.q && app.services.length}
      <div class="hint"><Icon name="star" size={14} /> Pin the services you care about most with the star or <kbd>p</kbd>. They stay on top here, in the sidebar and in the log sources.</div>
    {/if}
  {/snippet}
  {#snippet row(s)}
    {@const [cls, state] = svcState(s)}
    {@const e = errs(s)}
    <td class="pincell"><PinButton key={s.key} /></td>
    <td><b>{s.name}</b></td>
    <td class="muted">{s.stack}</td>
    <td><span class="pill {cls}">{s.running}/{s.desired}</span></td>
    <td><span class="state {cls}">{state}</span></td>
    <td class="hide-sm mono clip" title={s.image}>{s.image}</td>
    <td class="hide-sm mono muted">{s.ports.join(', ') || '–'}</td>
    <td class="num hide-sm mono">{pct(s.cpuPercent, 1)}</td>
    <td class="num hide-sm mono">{bytes(s.memUsed)}</td>
    <td class="num mono" style:color={e ? 'var(--err)' : 'var(--muted)'}>{e}</td>
    <td class="collectcell"><CollectSwitch names={[s.fullName]} label={s.fullName} /></td>
    <td class="hide-sm muted">{s.updatedAt ? ago(s.updatedAt, app.now) : '–'}</td>
  {/snippet}
</ResourceList>
