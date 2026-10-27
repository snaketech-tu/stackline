<script lang="ts">
  // The error trend in the "Errors · last hour" tile: 12 hours in 24 buckets.
  let { values }: { values: number[] } = $props()

  let pts = $derived.by(() => {
    const m = Math.max(1, ...values)
    const n = Math.max(2, values.length)
    return values.map((v, i) => [+(i * (88 / (n - 1))).toFixed(1), +(40 - (v / m) * 36).toFixed(1)])
  })
  let line = $derived(pts.map((p) => p.join(',')).join(' '))
</script>

<svg class="vis" width="92" height="44" viewBox="-2 0 92 44" aria-hidden="true">
  <defs>
    <linearGradient id="kg" x1="0" x2="0" y1="0" y2="1">
      <stop offset="0" stop-color="var(--err)" stop-opacity=".35" /><stop offset="1" stop-color="var(--err)" stop-opacity="0" />
    </linearGradient>
  </defs>
  {#if pts.length}
    <polygon points="0,42 {line} 88,42" fill="url(#kg)" />
    <polyline points={line} fill="none" stroke="var(--err)" stroke-width="1.8" stroke-linejoin="round" />
    <circle cx="88" cy={pts[pts.length - 1][1]} r="3" fill="var(--err)" />
  {/if}
</svg>
