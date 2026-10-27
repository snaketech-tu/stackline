<script lang="ts">
  // One picker for stacks and services together. Several can be on at once; any of them matches.
  import { app } from '../lib/state.svelte'
  import Icon from './Icon.svelte'
  import Popover from './Popover.svelte'

  const L = app.logs
  let btn: HTMLButtonElement | undefined = $state()
  let open = $state(false)
  let find = $state('')

  let needle = $derived(find.trim().toLowerCase())
  const hit = (s: string) => !needle || s.toLowerCase().includes(needle)
  let stacks = $derived((app.snap?.stacks ?? []).map((s) => s.name).filter(hit))
  let pinned = $derived(app.pinnedServices.filter((s) => hit(s.fullName)))
  let rest = $derived(app.services.filter((s) => !app.isPinned(s.key) && hit(s.fullName)).sort((a, b) => a.fullName.localeCompare(b.fullName)))
  let label = $derived.by(() => {
    const n = L.sources.size
    if (!n) return 'All sources'
    if (n === 1) {
      const [s] = L.sources
      return s.startsWith('stack:') ? `Stack ${s.slice(6)}` : s
    }
    return `${n} sources`
  })
  const errs = (full: string) => app.errors?.byService[full] ?? 0
</script>

<button class="ctl" bind:this={btn} aria-haspopup="menu" aria-expanded={open} onclick={() => { open = !open; find = '' }}>
  <Icon name="stacks" size={14} /><span style="max-width:200px;overflow:hidden;text-overflow:ellipsis" style:font-weight={L.sources.size ? 600 : 400}>{label}</span><Icon name="chevD" size={13} />
</button>
{#if open && btn}
  <Popover anchor={btn} label="Sources" onclose={() => (open = false)}>
    <!-- svelte-ignore a11y_autofocus -->
    <input class="find" type="search" placeholder="Find a stack or service" aria-label="Find a stack or service" bind:value={find} autofocus />
    {#if pinned.length}
      <div class="hd">★ Pinned</div>
      {#each pinned as s (s.key)}{@render item(s.fullName, s.fullName, errs(s.fullName))}{/each}
    {/if}
    {#if stacks.length}
      <div class="hd">Stacks</div>
      {#each stacks as st (st)}{@render item(`stack:${st}`, st, null, true)}{/each}
    {/if}
    {#if rest.length}
      <div class="hd">Services</div>
      {#each rest as s (s.key)}{@render item(s.fullName, s.fullName, errs(s.fullName))}{/each}
    {/if}
    {#if !pinned.length && !stacks.length && !rest.length}<div class="note">Nothing matches.</div>{/if}
    <div class="sepl"></div>
    <button class="it" role="menuitem" onclick={() => { L.sources.clear(); open = false }}>All sources</button>
  </Popover>
{/if}

{#snippet item(key: string, text: string, n: number | null, stack = false)}
  {@const on = L.sources.has(key)}
  <button class="it" role="menuitemcheckbox" aria-checked={on} onclick={() => L.toggleSource(key)}>
    <span class="cb" class:on>{#if on}<Icon name="check" size={11} />{/if}</span>
    <span class="l">{#if stack}<b>{text}</b> <span class="muted">all services</span>{:else}{text}{/if}</span>
    {#if n}<span class="r" style="color:var(--err)">{n} err</span>{/if}
  </button>
{/snippet}
