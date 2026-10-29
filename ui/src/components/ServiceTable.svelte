<script lang="ts">
  // Services in one table: flat, or with a header row per stack so the columns
  // line up across stacks. Hover a row for its logs and errors.
  import { SvelteSet } from 'svelte/reactivity'
  import { app } from '../lib/state.svelte'
  import { ago, bytes, pct, plural } from '../lib/format'
  import { stackState, svcState } from '../lib/health'
  import type { ServiceRef, Stack } from '../lib/types'
  import Icon from './Icon.svelte'
  import PinButton from './PinButton.svelte'
  import CollectSwitch from './CollectSwitch.svelte'

  type Group = { name: string; kind: Stack['kind']; total: number; services: ServiceRef[] }
  let { services = [], groups, stackCol = false, full = true, collectCol = false }: {
    services?: ServiceRef[]; groups?: Group[]; stackCol?: boolean
    /** Show mode, image and ports too. */
    full?: boolean
    /** Show the switches that choose whose logs are collected. */
    collectCol?: boolean
  } = $props()

  const closed = new SvelteSet<string>()
  let cols = $derived(8 + (stackCol ? 1 : 0) + (full ? 3 : 0) + (collectCol ? 1 : 0))

  function toggle(name: string) {
    if (closed.has(name)) closed.delete(name)
    else closed.add(name)
  }
</script>

<div class="tw">
  <table>
    <thead>
      <tr>
        <th class="pincell"><span class="sr">Pin</span></th><th>Service</th>{#if stackCol}<th>Stack</th>{/if}<th>Replicas</th>
        {#if full}<th class="hide-sm">Mode</th><th class="hide-sm">Image</th><th class="hide-sm">Ports</th>{/if}
        <th class="hide-sm">CPU</th><th class="hide-sm">Memory</th><th>Errors 1h</th>{#if collectCol}<th class="collectcell" title="Whether Stackline collects and stores the logs of the service">Collect</th>{/if}<th class="hide-sm">Updated</th><th class="actcell hide-sm"><span class="sr">Actions</span></th>
      </tr>
    </thead>
    <tbody>
      {#if groups}
        {#each groups as g (g.name)}
          {@const isClosed = closed.has(g.name)}
          {@const state = stackState(app.services.filter((s) => s.stack === g.name))}
          <tr class="grouphead" class:closed={isClosed}>
            <td colspan={cols}>
              <div class="gh">
                <button class="tg" aria-expanded={!isClosed} onclick={() => toggle(g.name)}><Icon name="chevD" size={14} />{g.name}</button>
                <span class="kind">{g.kind}</span>
                <span class="pill {state.cls}">{state.label}</span>{#if state.stopped}<span class="muted stopped">{state.stopped} stopped</span>{/if}
                <span class="muted hide-sm" style="font-size:12.5px">{g.services.length === g.total ? plural(g.total, 'service') : `${g.services.length} of ${g.total} services`}</span>
                <span class="spacer"></span>
                {#if collectCol}<span class="toggle" title="Collect the logs of every service in {g.name}"><span class="hide-sm">Whole stack</span><CollectSwitch names={g.services.map((s) => s.fullName)} label={g.name} /></span>{/if}
                <button class="btn ghost sm" title="Stack logs for {g.name}" aria-label="Stack logs for {g.name}" onclick={() => app.showLogs([`stack:${g.name}`])}><Icon name="logs" size={14} /><span class="hide-sm">Stack logs</span></button>
              </div>
            </td>
          </tr>
          {#if !isClosed}{#each g.services as s (s.key)}{@render row(s)}{/each}{/if}
        {:else}
          <tr><td colspan={cols} class="empty">No services match.</td></tr>
        {/each}
      {:else}
        {#each services as s (s.key)}{@render row(s)}{/each}
      {/if}
    </tbody>
  </table>
</div>

{#snippet row(s: ServiceRef)}
  {@const [cls] = svcState(s)}
  {@const e = app.errors?.byService[s.fullName] ?? 0}
  <tr class="click" class:pinned={app.isPinned(s.key)} onclick={() => app.openService(s.key)}>
    <td class="pincell"><PinButton key={s.key} /></td>
    <td><b>{s.name}</b></td>
    {#if stackCol}<td class="muted">{s.stack}</td>{/if}
    <td><span class="pill {cls}">{s.running}/{s.desired}</span></td>
    {#if full}
      <td class="hide-sm muted">{s.mode}</td>
      <td class="hide-sm mono" style="max-width:260px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap" title={s.image}>{s.image}</td>
      <td class="hide-sm mono muted">{s.ports.join(', ') || '–'}</td>
    {/if}
    <td class="hide-sm mono">{pct(s.cpuPercent, 1)}</td>
    <td class="hide-sm mono">{bytes(s.memUsed)}</td>
    <td class="mono" style:color={e ? 'var(--err)' : 'var(--muted)'}>{e}</td>
    {#if collectCol}<td class="collectcell"><CollectSwitch names={[s.fullName]} label={s.fullName} /></td>{/if}
    <td class="hide-sm muted">{s.updatedAt ? ago(s.updatedAt, app.now) : '–'}</td>
    <td class="actcell hide-sm">
      <span class="acts">
        <button class="ib" title="Logs of {s.fullName}" aria-label="Logs of {s.fullName}" onclick={(ev) => { ev.stopPropagation(); app.showLogs([s.fullName]) }}><Icon name="logs" size={15} /></button>
        <button class="ib" title="Errors of {s.fullName}" aria-label="Errors of {s.fullName}" onclick={(ev) => { ev.stopPropagation(); app.openService(s.key, 'errors') }}><Icon name="filter" size={15} /></button>
      </span>
    </td>
  </tr>
{/snippet}
