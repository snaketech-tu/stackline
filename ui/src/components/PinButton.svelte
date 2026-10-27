<script lang="ts">
  import { app } from '../lib/state.svelte'
  import Icon from './Icon.svelte'

  let { key, label = false }: { key: string; label?: boolean } = $props()
  let pinned = $derived(app.isPinned(key))
  const name = $derived(key.split('/').slice(1).join('/'))
</script>

<button type="button" class="pin" class:on={pinned} class:btn={label} class:ghost={label} aria-pressed={pinned}
  title={pinned ? `Unpin ${name}` : `Pin ${name} to the top`}
  onclick={(e) => { e.stopPropagation(); app.togglePin(key) }}>
  <Icon name="star" size={label ? 15 : 16} fill={pinned} />
  {#if label}{pinned ? 'Pinned' : 'Pin'}{/if}
</button>
