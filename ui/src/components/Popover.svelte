<script lang="ts">
  // A menu that opens under a button and closes on Escape or a click outside it.
  import type { Snippet } from 'svelte'
  import { onMount } from 'svelte'

  let { anchor, onclose, align = 'left', label, children }: {
    anchor: HTMLElement; onclose: () => void; align?: 'left' | 'right'; label: string; children: Snippet
  } = $props()

  let el: HTMLDivElement | undefined = $state()
  let pos = $state({ left: 0, top: 0 })

  function place() {
    if (!el) return
    const r = anchor.getBoundingClientRect()
    const w = el.offsetWidth
    const left = align === 'right' ? r.right - w : r.left
    pos = { left: Math.max(12, Math.min(left, innerWidth - w - 12)), top: r.bottom + 6 }
  }

  onMount(() => {
    // Live at the end of the page, so no card or panel can be painted over the menu
    // (the blurred top bar would otherwise trap it in its own layer).
    if (el) document.body.appendChild(el)
    place()
    const away = (e: PointerEvent) => {
      if (el && !el.contains(e.target as Node) && !anchor.contains(e.target as Node)) onclose()
    }
    const key = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation()
        onclose()
        anchor.focus()
      }
    }
    document.addEventListener('pointerdown', away, true)
    document.addEventListener('keydown', key, true)
    window.addEventListener('resize', place)
    return () => {
      document.removeEventListener('pointerdown', away, true)
      document.removeEventListener('keydown', key, true)
      window.removeEventListener('resize', place)
      el?.remove()
    }
  })
</script>

<!-- The menu itself moves to the end of the page; this wrapper stays where Svelte expects it. -->
<div style="display:contents">
  <div class="pop" role="menu" aria-label={label} bind:this={el} style:left="{pos.left}px" style:top="{pos.top}px">
    {@render children()}
  </div>
</div>
