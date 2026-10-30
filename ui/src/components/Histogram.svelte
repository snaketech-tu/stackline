<script lang="ts">
  // Stacked log volume per level with the exception count as a line on top.
  // Drag across it to zoom into a time range.
  import type { Bucket, DockerEvent, Level } from '../lib/types'
  import { hhmm, dayHhmm } from '../lib/format'
  import { eventColor, eventLabel, eventWho, isCrash } from '../lib/events'

  let {
    buckets,
    from,
    to,
    levels = ['DEBUG', 'INFO', 'WARN', 'ERROR'],
    exc = true,
    height = 150,
    marks = [],
    onrange,
  }: {
    buckets: Bucket[]
    from: number
    to: number
    levels?: Level[]
    exc?: boolean
    height?: number
    /** Container starts, stops and crashes, drawn as vertical lines. */
    marks?: DockerEvent[]
    onrange?: (from: number, to: number) => void
  } = $props()

  const COL: Record<Level, string> = { ERROR: 'var(--err)', WARN: 'var(--warn)', INFO: 'var(--info)', DEBUG: 'var(--dbg)' }
  const KEY: Record<Level, keyof Bucket> = { ERROR: 'error', WARN: 'warn', INFO: 'info', DEBUG: 'debug' }
  const W = 1000
  const uid = Math.random().toString(36).slice(2, 8)

  let H = $derived(height)
  let n = $derived(Math.max(1, buckets.length))
  let bw = $derived(W / n)
  let w = $derived((to - from) / n)
  let max = $derived.by(() => {
    let m = Math.max(1, ...buckets.map((b) => levels.reduce((a, l) => a + b[KEY[l]], 0)))
    const step = Math.pow(10, Math.floor(Math.log10(m)))
    m = Math.ceil(m / step) * step
    return Math.max(4, m)
  })
  let bars = $derived.by(() => {
    const out: { x: number; y: number; h: number; fill: string; op: number }[] = []
    buckets.forEach((b, i) => {
      let y = H
      for (const l of levels) {
        const v = b[KEY[l]]
        if (!v) continue
        const h = (v / max) * H
        y -= h
        out.push({ x: i * bw + bw * 0.14, y, h: Math.max(0, h - 0.6), fill: COL[l], op: l === 'ERROR' ? 0.92 : l === 'WARN' ? 0.85 : 0.38 })
      }
    })
    return out
  })
  let pts = $derived(buckets.map((b, i) => `${(i * bw + bw / 2).toFixed(1)},${(H - (b.exc / max) * H).toFixed(1)}`).join(' '))
  let anyExc = $derived(exc && buckets.some((b) => b.exc > 0))
  let lines = $derived(
    marks
      .filter((e) => e.ts / 1000 >= from && e.ts / 1000 <= to)
      .map((e) => ({ x: (((e.ts / 1000 - from) / (to - from)) * W).toFixed(1), color: eventColor(e), crash: isCrash(e) })),
  )
  const tipMarks = (i: number) => marks.filter((e) => e.ts / 1000 >= from + i * w && e.ts / 1000 < from + (i + 1) * w)
  const clock = (ts: number) => new Date(ts / 1000).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false })
  // Seconds on short ranges, such as the two minutes before a crash.
  const tick = (t: number) => (to - from < 10 * 60e3 ? clock(t * 1000) : dayHhmm(t, to - from))
  let ticks = $derived([0, 1, 2, 3, 4, 5].map((k) => tick(from + ((to - from) * k) / 5)))

  let svg: SVGSVGElement | undefined = $state()
  let host: HTMLDivElement | undefined = $state()
  let tip = $state<{ i: number; left: number; top: number } | null>(null)
  let drag = $state<number | null>(null)
  let brush = $state<[number, number] | null>(null)

  const frac = (ev: PointerEvent) => {
    const r = svg!.getBoundingClientRect()
    return Math.min(1, Math.max(0, (ev.clientX - r.left) / r.width))
  }

  function move(ev: PointerEvent) {
    const f = frac(ev)
    if (drag !== null) {
      brush = [Math.min(drag, f), Math.max(drag, f)]
      tip = null
      return
    }
    const hr = host!.getBoundingClientRect()
    const i = Math.min(n - 1, Math.floor(f * n))
    const tw = tipMarks(i).length ? 300 : 170
    let left = ev.clientX - hr.left + 14
    if (left > hr.width - tw) left = ev.clientX - hr.left - tw
    tip = { i, left, top: ev.clientY - hr.top - 10 }
  }

  function down(ev: PointerEvent) {
    if (!onrange) return
    drag = frac(ev)
    svg!.setPointerCapture(ev.pointerId)
  }

  function up(ev: PointerEvent) {
    if (drag === null) return
    const f = frac(ev)
    const a = Math.min(drag, f)
    const z = Math.max(drag, f)
    drag = null
    brush = null
    if (z - a > 0.01) onrange?.(from + (to - from) * a, from + (to - from) * z)
    else {
      const i = Math.min(n - 1, Math.floor(f * n))
      onrange?.(from + i * w, from + (i + 1) * w)
    }
  }
</script>

<div class="hist" bind:this={host}>
  <div class="yax" style:height="{H}px"><span>{max}</span><span>{max / 2}</span><span>0</span></div>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <svg class="plot" bind:this={svg} viewBox="0 0 {W} {H}" preserveAspectRatio="none" role="img" aria-label="Log volume over time"
    style:height="{H}px" style:cursor={onrange ? 'crosshair' : 'default'}
    onpointermove={move} onpointerleave={() => (tip = null)} onpointerdown={down} onpointerup={up}>
    {#each [0.25, 0.5, 0.75] as f (f)}
      <line x1="0" x2={W} y1={H * f} y2={H * f} stroke="var(--line)" stroke-width="1" stroke-dasharray="3 4" vector-effect="non-scaling-stroke" />
    {/each}
    <line x1="0" x2={W} y1={H} y2={H} stroke="var(--line)" vector-effect="non-scaling-stroke" />
    {#each bars as b, i (i)}
      <rect x={b.x.toFixed(1)} y={b.y.toFixed(1)} width={(bw * 0.72).toFixed(1)} height={b.h.toFixed(1)} rx="1.5" fill={b.fill} opacity={b.op} />
    {/each}
    {#if anyExc}
      <defs>
        <linearGradient id="xg-{uid}" x1="0" x2="0" y1="0" y2="1">
          <stop offset="0" stop-color="var(--exc)" stop-opacity=".28" /><stop offset="1" stop-color="var(--exc)" stop-opacity="0" />
        </linearGradient>
      </defs>
      <polygon points="{bw / 2},{H} {pts} {W - bw / 2},{H}" fill="url(#xg-{uid})" />
      <polyline points={pts} fill="none" stroke="var(--exc)" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" vector-effect="non-scaling-stroke" />
    {/if}
    {#each lines as m, i (i)}
      <line x1={m.x} x2={m.x} y1="0" y2={H} stroke={m.color} stroke-width={m.crash ? 1.6 : 1} stroke-dasharray={m.crash ? undefined : '3 3'}
        opacity={m.crash ? 0.9 : 0.6} vector-effect="non-scaling-stroke" />
      {#if m.crash}<line x1={m.x} x2={m.x} y1="0" y2="7" stroke={m.color} stroke-width="5" stroke-linecap="round" vector-effect="non-scaling-stroke" />{/if}
    {/each}
    {#if brush}
      <rect x={brush[0] * W} y="0" width={(brush[1] - brush[0]) * W} height={H} fill="var(--accent)" opacity=".15" />
    {/if}
  </svg>
  <div class="xax">{#each ticks as t, i (i)}<span>{t}</span>{/each}</div>
  {#if tip && buckets[tip.i]}
    {@const b = buckets[tip.i]}
    {@const evs = tipMarks(tip.i)}
    <div class="tip" style:left="{tip.left}px" style:top="{tip.top}px">
      <b>{hhmm(from + tip.i * w)} – {hhmm(from + (tip.i + 1) * w)}</b>
      {#each [...levels].reverse() as l (l)}
        <div><span style:color={COL[l]}>{l}</span><span class="mono">{b[KEY[l]]}</span></div>
      {/each}
      {#if exc}<div><span style:color="var(--exc)">Exceptions</span><span class="mono">{b.exc}</span></div>{/if}
      {#each evs.slice(-4) as e, i (i)}
        <div class="ev"><span style:color={eventColor(e)}>{clock(e.ts)} {eventWho(e)}</span><span>{eventLabel(e)}</span></div>
      {/each}
      {#if evs.length > 4}<span class="muted" style="font-size:11px">and {evs.length - 4} more</span>{/if}
      {#if onrange}<span class="muted" style="font-size:11px;margin-top:4px">Drag to zoom</span>{/if}
    </div>
  {/if}
</div>
