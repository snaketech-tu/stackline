<script lang="ts">
  // Switches log collection of one service (or several, such as a stack) on or off.
  import { app } from '../lib/state.svelte'

  let { names, label }: { names: string[]; label: string } = $props()
  let on = $derived(names.length > 0 && names.every((n) => app.collects(n)))
  let some = $derived(!on && names.some((n) => app.collects(n)))
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
<label class="switch sm" class:some title={on ? `Logs of ${label} are collected` : some ? `Logs of part of ${label} are collected` : `Logs of ${label} are not collected`}
  onclick={(e) => e.stopPropagation()}>
  <input type="checkbox" checked={on} aria-label="Collect logs of {label}" onchange={(e) => app.setCollects(names, e.currentTarget.checked)} /><i></i>
</label>
