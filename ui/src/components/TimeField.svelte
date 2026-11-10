<script lang="ts">
  // One moment of a time range: typed as text (`2026-11-10 08:15`, `10.11. 8:15`,
  // `yesterday 14:00`, `14:00`) or picked from the calendar beside it. Text
  // is the main way in: a plain field never fights the keyboard the way
  // segmented date fields do, and it reads the same words as the range box.
  import { fieldValue, inputValue, parsePoint, pointText } from '../lib/timeframe'
  import Icon from './Icon.svelte'

  let { value = $bindable(''), label, empty = '', onenter, oninput }: {
    value?: string
    label: string
    /** What an empty field means, such as "now". Without it the field is required. */
    empty?: string
    onenter?: () => void
    oninput?: () => void
  } = $props()

  const id = $props.id()
  let picker: HTMLInputElement | undefined = $state()
  let text: HTMLInputElement | undefined = $state()

  let at = $derived(value.trim() ? parsePoint(value) : null)
  /** What the field means right now, shown under it. */
  let meaning = $derived(!value.trim() ? (empty ? `Empty: ${empty}` : 'Type a date and time') : at === null ? 'Not a time yet' : pointText(at))

  function openPicker() {
    if (!picker) return
    picker.value = inputValue(at ?? Date.now())
    try {
      // The browser's own calendar and clock, where it has one.
      picker.showPicker()
    } catch {
      picker.focus()
      picker.click()
    }
  }

  function picked() {
    if (!picker?.value) return
    value = fieldValue(new Date(picker.value).getTime())
    oninput?.()
    text?.focus()
  }

  function key(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault()
      onenter?.()
    } else if ((e.key === 'ArrowUp' || e.key === 'ArrowDown') && at !== null) {
      // Up and down move by a minute, with Shift by an hour.
      e.preventDefault()
      const step = (e.shiftKey ? 60 : 1) * 60e3 * (e.key === 'ArrowUp' ? 1 : -1)
      value = fieldValue(at + step)
      oninput?.()
    }
  }
</script>

<div class="tf">
  <div class="field">
    <label for={id}>{label}</label>
    <span class="box">
      <input bind:this={text} {id} class="mono" bind:value placeholder="2026-11-10 08:15" spellcheck="false" autocomplete="off"
        aria-invalid={!!value.trim() && at === null} onkeydown={key} oninput={() => oninput?.()} />
      <button type="button" class="cal" onclick={openPicker} aria-label="Calendar for {label.toLowerCase()}" title="Pick from a calendar">
        <Icon name="calendar" size={14} />
      </button>
      <!-- The browser's picker opens from this field; it is never shown itself. -->
      <input bind:this={picker} class="ghost" type="datetime-local" step="60" tabindex="-1" aria-hidden="true" onchange={picked} />
    </span>
  </div>
  <span class="mean" class:bad={!!value.trim() && at === null}>{meaning}</span>
</div>

<style>
  .tf{display:grid;gap:3px;min-width:0}
  .field{min-width:0}
  .box{position:relative;display:flex;align-items:center;min-width:0}
  .box .mono{height:34px;font-size:12.5px;min-width:0;width:100%;padding:0 34px 0 8px}
  .box .mono[aria-invalid="true"]{border-color:var(--err)}
  .cal{position:absolute;right:3px;top:50%;transform:translateY(-50%);width:28px;height:28px;display:grid;place-items:center;border:0;border-radius:6px;background:transparent;color:var(--muted);cursor:pointer}
  .cal:hover{background:var(--panel-2);color:var(--fg)}
  .ghost{position:absolute;right:0;bottom:0;width:1px;height:1px;opacity:0;pointer-events:none;border:0;padding:0}
  .mean{font-size:11.5px;color:var(--muted);padding-left:2px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
  .mean.bad{color:var(--err)}
</style>
