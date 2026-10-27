<script lang="ts">
  // A progress ring for KPI tiles. `load` colours by how full it is (CPU),
  // otherwise by how healthy it is (ready / desired).
  let { value, total, load = false }: { value: number; total: number; load?: boolean } = $props()

  const c = 2 * Math.PI * 19
  let f = $derived(total ? Math.min(1, Math.max(0, value / total)) : 0)
  let col = $derived(
    load
      ? f > 0.85 ? 'var(--err)' : f > 0.7 ? 'var(--warn)' : 'var(--accent)'
      : f >= 0.999 ? 'var(--ok)' : f >= 0.75 ? 'var(--accent)' : f >= 0.5 ? 'var(--warn)' : 'var(--err)',
  )
</script>

<svg class="vis" width="52" height="52" viewBox="0 0 52 52" aria-hidden="true">
  <circle cx="26" cy="26" r="19" fill="none" stroke="var(--panel-2)" stroke-width="6" />
  <circle cx="26" cy="26" r="19" fill="none" stroke={col} stroke-width="6" stroke-linecap="round"
    stroke-dasharray="{(c * f).toFixed(1)} {c.toFixed(1)}" transform="rotate(-90 26 26)" />
  <text x="26" y="30" text-anchor="middle" font-size="11" font-weight="700" style="fill:var(--fg);font-family:var(--mono)">{Math.round(f * 100)}%</text>
</svg>
