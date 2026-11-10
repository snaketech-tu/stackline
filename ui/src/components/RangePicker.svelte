<script lang="ts">
  import { untrack } from 'svelte'
  // The time range of the Logs page: one-click presets, a menu to type or pick
  // any range, and steps to the range before or after.
  import { app, PRESETS, type Preset } from '../lib/state.svelte'
  import { fieldValue, parsePoint, parseRange, rangeInput, presetText, rangeText, type Range } from '../lib/timeframe'
  import Icon from './Icon.svelte'
  import Popover from './Popover.svelte'
  import TimeField from './TimeField.svelte'

  let { open = $bindable(false) }: { open?: boolean } = $props()

  const L = app.logs
  const BUTTONS: Preset[] = ['auto', ...PRESETS]
  const QUICK: [string, string][] = [
    ['5m', '5m'], ['30m', '30m'], ['6h', '6h'], ['12h', '12h'], ['2d', '2d'], ['3d', '3d'],
    ['Today', 'today'], ['Yesterday', 'yesterday'], ['This week', 'this week'],
  ]
  const EXAMPLES = ['2h', 'yesterday', '14:00-15:30', 'since 8:00', '9.11. 8:00 - 12:00', '14:05 ± 5m']

  let anchor: HTMLButtonElement | undefined = $state()
  let typed = $state('')
  let fromIn = $state('')
  let toIn = $state('')
  let error = $state('')

  /** A range that has no button of its own shows as words on the menu button. */
  let custom = $derived(!(L.preset && BUTTONS.includes(L.preset)))

  // Each time the menu opens, and only then, the fields start at the range on
  // screen. Untracked: the range moves with the clock and the store, and
  // reading it here would put the fields back while someone edits them.
  $effect(() => {
    if (open) untrack(fill)
  })
  function fill() {
    const [a, z] = L.window()
    typed = ''
    error = ''
    fromIn = fieldValue(a)
    toIn = L.moving ? '' : fieldValue(z)
  }

  function label(r: Range): string {
    if ('preset' in r) return r.preset === 'auto' ? 'all stored lines' : presetText(r.preset)
    return rangeText(r.from, r.to)
  }

  let preview = $derived.by(() => {
    if (!typed.trim()) return null
    const r = parseRange(typed)
    return r ? label(r) : ''
  })

  function applyTyped() {
    if (!typed.trim()) return
    if (L.applyText(typed)) open = false
    else error = 'That is not a range Stackline understands. Try one of the examples.'
  }

  function pick(text: string) {
    L.applyText(text)
    open = false
  }

  function applyFields() {
    const a = parsePoint(fromIn)
    const z = toIn.trim() ? parsePoint(toIn) : null
    if (a === null) return void (error = 'The start is not a time Stackline understands, such as 2026-11-10 08:15.')
    if (toIn.trim() && z === null) return void (error = 'The end is not a time Stackline understands, such as 2026-11-10 09:30.')
    if (z !== null && z <= a) return void (error = 'The end must come after the start.')
    if (a >= Date.now()) return void (error = 'The start is in the future.')
    L.setRange({ from: a, to: z })
    L.remember(rangeInput(a, z))
    open = false
  }

  function toggle() {
    open = !open
  }
</script>

<div class="rp">
  <div class="seg" role="group" aria-label="Time range">
    {#each BUTTONS as p (p)}
      <button aria-pressed={L.preset === p} onclick={() => L.setPreset(p)} title={p === 'auto' ? 'From the oldest stored line until now' : presetText(p)}>{p === 'auto' ? 'Auto' : p}</button>
    {/each}
  </div>
  <button class="ctl rbtn" class:on={custom} bind:this={anchor} aria-haspopup="dialog" aria-expanded={open} onclick={toggle}
    title="Type or pick any time range (t)">
    <Icon name="clock" size={14} /><span class="rl">{custom ? L.rangeLabel(app.now) : 'Custom'}</span><Icon name="chevD" size={12} />
  </button>
  <span class="steps">
    <button class="ctl icon" onclick={() => L.step(-1)} aria-label="Earlier" title="Earlier: the same length before this range ( [ )"><Icon name="chevL" size={14} /></button>
    <button class="ctl icon" onclick={() => L.step(1)} disabled={L.moving} aria-label="Later" title="Later: the same length after this range ( ] )"><Icon name="chev" size={14} /></button>
  </span>
</div>

{#if open && anchor}
  <Popover {anchor} label="Time range" wide onclose={() => (open = false)}>
    <div class="hd">Type a range</div>
    <!-- svelte-ignore a11y_autofocus -->
    <input class="find mono" bind:value={typed} autofocus placeholder="2h, yesterday, 14:00-15:30, since 8:00…" spellcheck="false" autocomplete="off"
      aria-label="Type a time range" onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); applyTyped() } }} oninput={() => (error = '')} />
    {#if preview}
      <button class="it" onclick={applyTyped}><Icon name="check" size={13} /><span class="l">Show {preview}</span><span class="r">Enter</span></button>
    {:else if preview === ''}
      <div class="note err">Not a range yet. Examples: {EXAMPLES.join(' · ')}</div>
    {:else}
      <div class="note">For example {#each EXAMPLES as ex, i (ex)}{#if i}{', '}{/if}<button class="ex mono" onclick={() => (typed = ex)}>{ex}</button>{/each}</div>
    {/if}

    <div class="sepl"></div>
    <div class="hd">Quick</div>
    <div class="chips">
      {#each QUICK as [name, text] (text)}
        <button class="btn sm" onclick={() => pick(text)}>{name}</button>
      {/each}
    </div>

    <div class="sepl"></div>
    <div class="hd">From and to</div>
    <div class="fields">
      <TimeField label="From" bind:value={fromIn} onenter={applyFields} oninput={() => (error = '')} />
      <TimeField label="To" bind:value={toIn} empty="until now" onenter={applyFields} oninput={() => (error = '')} />
    </div>
    <div class="row-end">
      {#if toIn}<button class="btn ghost sm" onclick={() => (toIn = '')}>Until now</button>{/if}
      <button class="btn pri sm" onclick={applyFields}>Show this range</button>
    </div>
    {#if error}<div class="note err" role="alert">{error}</div>{/if}

    {#if L.recent.length}
      <div class="sepl"></div>
      <div class="hd">Recent</div>
      {#each L.recent as text (text)}
        {@const r = parseRange(text)}
        {#if r}
          <button class="it" onclick={() => pick(text)}><Icon name="clock" size={13} /><span class="l mono">{text}</span><span class="r">{label(r)}</span></button>
        {/if}
      {/each}
    {/if}
    <div class="sepl"></div>
    <div class="note">Drag across the graph to zoom in. <kbd>[</kbd> <kbd>]</kbd> step earlier or later, <kbd>-</kbd> zooms out, <kbd>t</kbd> opens this menu.</div>
  </Popover>
{/if}

<style>
  .rp{display:inline-flex;align-items:center;gap:6px;flex-wrap:wrap;min-width:0}
  .rbtn{max-width:min(320px,70vw)}
  .rbtn.on{border-color:var(--accent);background:var(--accent-soft);color:var(--fg);font-weight:600}
  .rl{min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:13px}
  .steps{display:inline-flex;gap:4px}
  .steps button:disabled{opacity:.4;cursor:default}
  .chips{display:flex;flex-wrap:wrap;gap:6px;padding:2px 10px 6px}
  .fields{display:grid;grid-template-columns:minmax(0,1fr) minmax(0,1fr);gap:8px;padding:2px 10px}
  .row-end{display:flex;justify-content:flex-end;align-items:center;gap:6px;padding:8px 10px 4px}
  .note.err{color:var(--err)}
  .ex{all:unset;cursor:pointer;color:var(--accent);font-size:11.5px}
  .ex:hover{text-decoration:underline}
  @media (max-width:520px){.fields{grid-template-columns:1fr}}
</style>
