<script lang="ts">
  import type { Snippet } from 'svelte'
  import { app } from '../lib/state.svelte'
  import Icon from './Icon.svelte'

  let { title, children, actions, sub, tabs, foot, size = 'normal' }: {
    title: string; children: Snippet; actions?: Snippet; sub?: Snippet; tabs?: Snippet; foot?: Snippet
    /** `wide` leaves a strip of the page visible, `full` covers it. */
    size?: 'normal' | 'wide' | 'full'
  } = $props()
  let heading: HTMLHeadingElement | undefined = $state()

  // Focus the title, so keyboard users start in the drawer without a ring on a button.
  $effect(() => {
    heading?.focus()
  })
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="scrim" onclick={() => (app.drawer = null)}></div>
<div class="drawer" class:wide={size === 'wide'} class:full={size === 'full'} role="dialog" aria-modal="true" aria-label={title}>
  <div class="dh" style:border-bottom={sub || tabs ? '0' : undefined} style:padding-bottom={sub || tabs ? '4px' : undefined}>
    <h3 tabindex="-1" bind:this={heading}>{title}</h3>
    {@render actions?.()}
    <button class="ib" title="Close (Esc)" aria-label="Close" onclick={() => (app.drawer = null)}><Icon name="x" size={16} /></button>
  </div>
  {#if sub}<div class="dsub">{@render sub()}</div>{/if}
  {#if tabs}<div class="tabs" role="tablist">{@render tabs()}</div>{/if}
  <div class="db">{@render children()}</div>
  {#if foot}<div class="dfoot">{@render foot()}</div>{/if}
</div>
