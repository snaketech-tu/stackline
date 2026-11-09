<script lang="ts">
  // The log workspace: tabs of log streams that keep running at the bottom
  // of the window while the lists and inspector are used above it.
  import { app } from '../lib/state.svelte'
  import Icon from './Icon.svelte'
  import DockLog from './DockLog.svelte'

  const D = app.dock
  let active = $derived(D.tabs.find((t) => t.id === D.active) ?? D.tabs[0])

  function grab(e: PointerEvent) {
    if (D.min) return
    const y0 = e.clientY
    const h0 = D.height
    const el = e.currentTarget as HTMLElement
    el.setPointerCapture(e.pointerId)
    const move = (ev: PointerEvent) => D.resize(h0 + (y0 - ev.clientY))
    const up = () => {
      el.removeEventListener('pointermove', move)
      el.removeEventListener('pointerup', up)
    }
    el.addEventListener('pointermove', move)
    el.addEventListener('pointerup', up)
  }
  function resizeKey(e: KeyboardEvent) {
    if (e.key === 'ArrowUp') D.resize(D.height + 24)
    else if (e.key === 'ArrowDown') D.resize(D.height - 24)
    else return
    e.preventDefault()
  }
</script>

<section class="dock" class:min={D.min} style:height={D.min ? undefined : `${D.height}px`} aria-label="Log workspace">
  <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
  <div class="dock-grip" role="separator" aria-orientation="horizontal" aria-label="Height of the log workspace" tabindex="0" onpointerdown={grab} onkeydown={resizeKey}></div>
  <div class="dock-tabs" role="tablist">
    {#each D.tabs as t (t.id)}
      <div class="dtab" class:on={active?.id === t.id}>
        <button role="tab" aria-selected={active?.id === t.id} onclick={() => D.pick(t.id)} title={t.label}><Icon name="logs" size={13} /><span class="t">{t.label}</span></button>
        <button class="x" onclick={() => D.close(t.id)} aria-label="Close {t.label}" title="Close"><Icon name="x" size={11} /></button>
      </div>
    {/each}
    <span class="spacer"></span>
    {#if D.tabs.length > 1}<button class="ib" onclick={() => D.closeAll()} title="Close every tab" aria-label="Close every tab"><Icon name="x" size={14} /></button>{/if}
    <button class="ib" onclick={() => D.toggle()} title={D.min ? 'Show the workspace (w)' : 'Shrink the workspace to its tabs (w)'} aria-label={D.min ? 'Show the workspace' : 'Shrink the workspace'}><Icon name={D.min ? 'chevU' : 'chevD'} size={15} /></button>
  </div>
  {#if !D.min && active}
    {#key active.id + app.activeId}<div class="dock-body"><DockLog tab={active} /></div>{/key}
  {/if}
</section>
