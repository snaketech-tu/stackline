<script lang="ts">
  import type { Snippet } from 'svelte'
  import { app } from '../lib/state.svelte'

  let { title, children, actions }: { title: string; children: Snippet; actions?: Snippet } = $props()
  let closeBtn: HTMLButtonElement | undefined = $state()

  $effect(() => {
    closeBtn?.focus()
  })
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="scrim" onclick={() => (app.drawer = null)}></div>
<div class="drawer" role="dialog" aria-modal="true" aria-label={title}>
  <div class="dh">
    <h3>{title}</h3>
    {@render actions?.()}
    <button class="btn ghost" bind:this={closeBtn} onclick={() => (app.drawer = null)}>Close</button>
  </div>
  <div class="db">{@render children()}</div>
</div>
