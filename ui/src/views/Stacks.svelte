<script lang="ts">
  import { SvelteSet } from 'svelte/reactivity'
  import { app, pinKey } from '../lib/state.svelte'
  import { ago, bytes, pct, plural } from '../lib/format'
  import { svcState } from '../lib/health'
  import type { Service } from '../lib/types'
  import Icon from '../components/Icon.svelte'
  import PinButton from '../components/PinButton.svelte'
  import PinnedTable from '../components/PinnedTable.svelte'

  let snap = $derived(app.snap!)
  let filter = $state('')
  const closed = new SvelteSet<string>()

  let needle = $derived(filter.trim().toLowerCase())
  const matches = (s: Service) => !needle || s.name.toLowerCase().includes(needle) || s.image.toLowerCase().includes(needle) || s.stack.toLowerCase().includes(needle)

  /** Pinned services first, then by name. */
  function sorted(list: Service[]) {
    const pins = app.pins
    return list
      .filter(matches)
      .sort((a, b) => {
        const pa = pins.includes(pinKey(a)) ? 0 : 1
        const pb = pins.includes(pinKey(b)) ? 0 : 1
        return pa - pb || a.name.localeCompare(b.name)
      })
  }

  function toggle(name: string) {
    if (closed.has(name)) closed.delete(name)
    else closed.add(name)
  }

  function stackLogs(name: string) {
    app.logs.reset()
    app.logs.stack = name
    app.go('logs')
  }
</script>

<div class="panel pb fbar">
  <label class="search" style="flex:1">
    <Icon name="search" size={14} />
    <input placeholder="Filter services by name, image or stack" bind:value={filter} autocomplete="off" spellcheck="false" />
  </label>
  <span class="muted hide-sm" style="font-size:12.5px">{plural(app.services.length, 'service')} in {plural(snap.stacks.length, 'stack')}</span>
</div>

{#if app.pinnedServices.length}
  {@const pinned = app.pinnedServices.filter(matches)}
  {#if pinned.length}
    <section class="panel">
      <div class="ph"><h2><Icon name="star" size={15} fill class="star-ic" /> Pinned</h2><span class="muted" style="font-size:12px">{pinned.length}</span></div>
      <PinnedTable services={pinned} />
    </section>
  {/if}
{:else if !needle}
  <div class="hint" style="display:flex;gap:6px;align-items:center;padding:0 4px"><Icon name="star" size={14} /> Pin the services you care about most with the star. They stay at the top here and on the overview.</div>
{/if}

{#each snap.stacks as st (st.name)}
  {@const list = sorted(st.services)}
  {#if list.length}
    <section class="panel stack" class:closed={closed.has(st.name)}>
      <div class="ph" tabindex="0" role="button" aria-expanded={!closed.has(st.name)}
        onclick={(e) => { if (!(e.target as HTMLElement).closest('button')) toggle(st.name) }}
        onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); toggle(st.name) } }}>
        <Icon name="chevD" size={14} class="chev" />
        <h3>{st.name}</h3><span class="kind">{st.kind}</span>
        <span class="muted">{plural(st.services.length, 'service')}</span>
        <span class="spacer"></span>
        <button class="btn ghost" onclick={() => stackLogs(st.name)}>Logs</button>
      </div>
      <div class="tw">
        <table>
          <thead>
            <tr><th class="pincell"><span class="sr">Pin</span></th><th>Service</th><th>Replicas</th><th class="hide-sm">Mode</th><th>Image</th><th class="hide-sm">Ports</th><th class="hide-sm">CPU</th><th class="hide-sm">Memory</th><th>Errors 1h</th><th class="hide-sm">Updated</th></tr>
          </thead>
          <tbody>
            {#each list as s (s.id)}
              {@const key = pinKey(s)}
              {@const [cls] = svcState(s)}
              {@const e = app.errors?.byService[s.fullName] ?? 0}
              <tr class="click" class:pinned={app.isPinned(key)} onclick={() => (app.drawer = { kind: 'service', key })}>
                <td class="pincell"><PinButton {key} /></td>
                <td><b>{s.name}</b></td>
                <td><span class="pill {cls}">{s.running}/{s.desired}</span></td>
                <td class="hide-sm muted">{s.mode}</td>
                <td class="mono" style="max-width:280px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap" title={s.image}>{s.image}</td>
                <td class="hide-sm mono muted">{s.ports.join(', ') || '–'}</td>
                <td class="hide-sm mono">{pct(s.cpuPercent, 1)}</td>
                <td class="hide-sm mono">{bytes(s.memUsed)}</td>
                <td class="mono" style:color={e ? 'var(--err)' : 'var(--muted)'}>{e}</td>
                <td class="hide-sm muted">{s.updatedAt ? ago(s.updatedAt, app.now) : '–'}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    </section>
  {/if}
{:else}
  <div class="center-empty">No stacks or containers on this host.</div>
{/each}
