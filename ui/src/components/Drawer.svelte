<script lang="ts">
  // A panel for one thing: docked beside the list as the inspector (a service,
  // a node), or over the page for forms (a connection, an update).
  import type { Snippet } from 'svelte'
  import { app } from '../lib/state.svelte'
  import Icon from './Icon.svelte'

  let { title, kind, children, actions, sub, tabs, foot, size = 'normal', docked = false }: {
    title: string; children: Snippet; actions?: Snippet; sub?: Snippet; tabs?: Snippet; foot?: Snippet
    /** What the panel shows, said above its title, like "Service inspector". */
    kind?: string
    /** `wide` leaves a strip of the page visible, `full` covers it. Only for panels over the page. */
    size?: 'normal' | 'wide' | 'full'
    /** Beside the list, which stays usable, instead of over the page. */
    docked?: boolean
  } = $props()
  let heading: HTMLHeadingElement | undefined = $state()

  // A panel over the page takes the focus, so keyboard users start in it. A
  // docked inspector leaves it on the list, whose keys keep working.
  $effect(() => {
    if (!docked) heading?.focus()
  })
</script>

{#if !docked}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="scrim" onclick={() => (app.drawer = null)}></div>
{/if}
<svelte:element this={docked ? 'aside' : 'div'} class="drawer" class:docked class:wide={!docked && size === 'wide'} class:full={!docked && size === 'full'}
  role={docked ? 'complementary' : 'dialog'} aria-modal={docked ? undefined : 'true'} aria-label={title}>
  <div class="dh" style:border-bottom={sub || tabs ? '0' : undefined} style:padding-bottom={sub || tabs ? '4px' : undefined}>
    <div class="dtitle">
      {#if kind}<span class="eyebrow">{kind}</span>{/if}
      <h3 tabindex="-1" bind:this={heading}>{title}</h3>
    </div>
    {@render actions?.()}
    {#if docked}
      <button class="ib hide-sm" title={app.inspectorFull ? 'Show the list beside it' : 'Use the whole page'} aria-label={app.inspectorFull ? 'Show the list beside it' : 'Use the whole page'}
        onclick={() => app.setInspectorFull(!app.inspectorFull)}><Icon name={app.inspectorFull ? 'minimize' : 'maximize'} size={15} /></button>
    {/if}
    <button class="ib" title="Close (Esc)" aria-label="Close" onclick={() => (app.drawer = null)}><Icon name="x" size={16} /></button>
  </div>
  {#if sub}<div class="dsub">{@render sub()}</div>{/if}
  {#if tabs}<div class="tabs" role="tablist">{@render tabs()}</div>{/if}
  <div class="db">{@render children()}</div>
  {#if foot}<div class="dfoot">{@render foot()}</div>{/if}
</svelte:element>
