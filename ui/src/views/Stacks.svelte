<script lang="ts">
  import { app, type StackFilter } from '../lib/state.svelte'
  import { plural } from '../lib/format'
  import type { ServiceRef } from '../lib/types'
  import Icon from '../components/Icon.svelte'
  import ServiceTable from '../components/ServiceTable.svelte'

  let snap = $derived(app.snap!)
  let needle = $derived(app.stackQ.trim().toLowerCase())
  const matches = (s: ServiceRef) =>
    !needle || s.name.toLowerCase().includes(needle) || s.image.toLowerCase().includes(needle) || s.stack.toLowerCase().includes(needle)
  const problem = (s: ServiceRef) => s.running < s.desired || (s.tasks.some((t) => t.health === 'unhealthy'))
  const keep: Record<StackFilter, (s: ServiceRef) => boolean> = {
    all: () => true,
    problems: problem,
    pinned: (s) => app.isPinned(s.key),
  }
  let counts = $derived({
    all: app.services.length,
    problems: app.services.filter(problem).length,
    pinned: app.pinnedServices.length,
  })

  /** Pinned services first, then by name, grouped by stack. */
  let groups = $derived(
    snap.stacks
      .map((st) => ({
        name: st.name,
        kind: st.kind,
        total: st.services.length,
        services: app.services
          .filter((s) => s.stack === st.name && matches(s) && keep[app.stackFilter](s))
          .sort((a, b) => Number(app.isPinned(b.key)) - Number(app.isPinned(a.key)) || a.name.localeCompare(b.name)),
      }))
      .filter((g) => g.services.length),
  )
  let collected = $derived(app.services.filter((s) => app.collects(s.fullName)).length)
  let shown = $derived(groups.reduce((a, g) => a + g.services.length, 0))
  const FILTERS: [StackFilter, string][] = [['all', 'All'], ['problems', 'Problems'], ['pinned', 'Pinned']]
</script>

<div class="panel pb fbar">
  <div class="seg" role="group" aria-label="Show">
    {#each FILTERS as [k, label] (k)}
      <button aria-pressed={app.stackFilter === k} onclick={() => (app.stackFilter = k)}>{label}<span class="n">{counts[k]}</span></button>
    {/each}
  </div>
  <label class="search">
    <Icon name="search" size={14} />
    <input placeholder="Filter by service, image or stack" bind:value={app.stackQ} autocomplete="off" spellcheck="false" />
    {#if app.stackQ}<button class="ib" style="width:22px;height:22px" onclick={() => (app.stackQ = '')} aria-label="Clear filter"><Icon name="x" size={12} /></button>{/if}
  </label>
  <div class="seg" role="group" aria-label="Collect logs of" title="Which services swarmdash reads and stores logs for">
    <button aria-pressed={!app.collect.only} onclick={() => app.setCollectOnly(false)}>Collect all</button>
    <button aria-pressed={app.collect.only} onclick={() => app.setCollectOnly(true)}>Only chosen<span class="n">{collected}</span></button>
  </div>
  <span class="muted hide-sm" style="font-size:12.5px">{shown === app.services.length ? plural(shown, 'service') : `${shown} of ${app.services.length} services`} in {plural(snap.stacks.length, 'stack')}</span>
</div>

{#if !app.pinnedServices.length && app.stackFilter === 'all' && !needle && app.services.length}
  <div class="hint" style="display:flex;gap:6px;align-items:center;padding:0 4px"><Icon name="star" size={14} /> Pin the services you care about most with the star. They stay on top here, on the overview and in the log sources.</div>
{/if}

{#if app.collect.only && !collected && app.services.length}
  <div class="hint" style="display:flex;gap:6px;align-items:center;padding:0 4px"><Icon name="logs" size={14} /> No logs are collected right now. Switch on the services you want to follow in the Collect column.</div>
{:else if app.collect.only || app.services.some((s) => !app.collects(s.fullName))}
  <div class="hint" style="display:flex;gap:6px;align-items:center;padding:0 4px"><Icon name="logs" size={14} /> Logs are collected for {collected} of {plural(app.services.length, 'service')}. {app.collect.only ? 'New services are not collected until you switch them on.' : 'New services are collected.'}</div>
{/if}

{#if !snap.stacks.length}
  <div class="center-empty">No stacks or containers on this host.</div>
{:else}
  <section class="panel">
    {#if groups.length}<ServiceTable {groups} collectCol />{:else}
      <div class="empty">
        {app.stackFilter === 'problems' ? 'Every service runs as it should.' : app.stackFilter === 'pinned' ? 'No pinned services yet. Pin one with the star.' : 'No services match.'}
        {#if needle || app.stackFilter !== 'all'}<button class="btn ghost sm" onclick={() => { app.stackQ = ''; app.stackFilter = 'all' }}>Show all</button>{/if}
      </div>
    {/if}
  </section>
{/if}
