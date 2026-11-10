<script lang="ts">
  // The top bar's stack picker: narrows the lists to one stack, the way a
  // namespace picker narrows a Kubernetes view. "All stacks" shows everything.
  import { app } from '../lib/state.svelte'
  import { stackState } from '../lib/health'
  import Icon from './Icon.svelte'
  import Popover from './Popover.svelte'

  let btn: HTMLButtonElement | undefined = $state()
  let open = $state(false)
  let stacks = $derived(app.snap?.stacks ?? [])

  function pick(name: string) {
    open = false
    app.scope = name
  }
</script>

<button class="picker" bind:this={btn} aria-haspopup="menu" aria-expanded={open} onclick={() => (open = !open)} title="Narrow the lists to one stack">
  <span class="cap">Stack</span>
  <span class="val">{app.scope || 'All stacks'}</span>
  <Icon name="chevD" size={14} />
</button>
{#if open && btn}
  <Popover anchor={btn} label="Stacks" onclose={() => (open = false)}>
    <div class="hd">Show the lists for</div>
    <button class="it" class:sel={!app.scope} role="menuitemradio" aria-checked={!app.scope} onclick={() => pick('')}>
      <span class="l"><b>All stacks</b></span><span class="r">{stacks.length}</span>
    </button>
    {#each stacks as st (st.name)}
      {@const s = stackState(st.services)}
      <button class="it" class:sel={app.scope === st.name} role="menuitemradio" aria-checked={app.scope === st.name} onclick={() => pick(st.name)}>
        <span class="dot {s.cls === 'ok' ? '' : s.cls || 'off'}"></span>
        <span class="l">{st.name} <span class="muted" style="font-size:11px">{st.kind}</span></span>
        <span class="r">{st.services.length}</span>
      </button>
    {/each}
  </Popover>
{/if}
