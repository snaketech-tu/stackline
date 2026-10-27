<script lang="ts">
  import { ICON } from '../lib/icons'

  let { name, size = 16, fill = false, class: cls = '' }: { name: string; size?: number; fill?: boolean; class?: string } = $props()
</script>

<svg class={cls} width={size} height={size} viewBox="0 0 24 24" fill={fill ? 'currentColor' : 'none'} stroke="currentColor"
  stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">{@html ICON[name]}</svg>
