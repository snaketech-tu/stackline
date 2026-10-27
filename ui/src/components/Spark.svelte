<script lang="ts">
  // Tiny bar sparkline for an error group. Empty buckets are faint stubs, so a
  // single burst still reads as a chart.
  let { values, warn = false }: { values: number[]; warn?: boolean } = $props()
  let m = $derived(Math.max(1, ...values))
  let w = $derived(Math.max(1, values.length) * 2.5)
</script>

<svg width={w} height="16" viewBox="0 0 {w} 16" aria-hidden="true" style="flex:none">
  {#each values as v, i (i)}
    {@const h = v ? Math.max(2, (v / m) * 15) : 1.5}
    <rect x={i * 2.5} y={16 - h} width="1.6" height={h} rx="0.5" fill={v ? (warn ? 'var(--warn)' : 'var(--err)') : 'var(--line)'} />
  {/each}
</svg>
