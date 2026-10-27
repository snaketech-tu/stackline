<script lang="ts">
  // Tiny bar sparkline for an error group.
  let { values }: { values: number[] } = $props()
  let m = $derived(Math.max(1, ...values))
</script>

<svg width="72" height="18" viewBox="0 0 72 18" aria-hidden="true">
  {#each values as v, i (i)}
    {#if v}
      <rect x={i * 3} y={18 - (v / m) * 17} width="2" height={(v / m) * 17} fill="var(--err)" />
    {:else}
      <rect x={i * 3} y="17" width="2" height="1" fill="var(--line)" />
    {/if}
  {/each}
</svg>
