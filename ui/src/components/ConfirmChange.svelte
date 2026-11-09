<script lang="ts">
  // Asks before anything changes: what will happen, and the docker command it
  // amounts to. Enter confirms, Esc cancels. A failure stays in the dialog.
  import { app, type Ask } from '../lib/state.svelte'
  import Icon from './Icon.svelte'

  let { ask }: { ask: Ask } = $props()

  // svelte-ignore state_referenced_locally
  let n = $state(ask.replicas ?? 0)
  let busy = $state(false)
  let error = $state('')
  let confirmEl: HTMLButtonElement | undefined = $state()
  let numEl: HTMLInputElement | undefined = $state()

  let valid = $derived(ask.replicas === undefined || (Number.isInteger(n) && n >= 0 && n <= 100))
  let command = $derived(ask.scaleOf ? `docker service scale ${ask.scaleOf}=${valid ? n : '…'}` : ask.command)

  $effect(() => {
    if (numEl) {
      numEl.focus()
      numEl.select()
    } else confirmEl?.focus()
  })

  function close() {
    if (!busy) app.ask = null
  }

  async function go() {
    if (busy || !valid) return
    busy = true
    error = ''
    try {
      await ask.run(ask.replicas === undefined ? undefined : n)
      app.ask = null
    } catch (e) {
      error = app.describe(e)
    } finally {
      busy = false
    }
  }

  function keydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault()
      e.stopPropagation()
      close()
    } else if (e.key === 'Enter' && (e.target as HTMLElement).tagName !== 'BUTTON') {
      e.preventDefault()
      go()
    }
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="pal-wrap" onclick={(e) => { if (e.target === e.currentTarget) close() }} onkeydown={keydown}>
  <div class="pal ask-dlg" role="alertdialog" aria-modal="true" aria-labelledby="confirm-title">
    <div class="c-body">
      <h3 id="confirm-title">{ask.title}</h3>
      {#if ask.text}<p>{ask.text}</p>{/if}
      {#if ask.replicas !== undefined}
        <label class="c-num">Replicas
          <input bind:this={numEl} type="number" min="0" max="100" step="1" bind:value={n} aria-invalid={!valid} />
        </label>
        {#if !valid}<p class="c-err">Pick a whole number from 0 to 100.</p>{/if}
      {/if}
      {#if command}
        <div class="c-cmd"><span class="muted">Same as</span><code>{command}</code></div>
      {/if}
      {#if error}<p class="c-err" role="alert">{error}</p>{/if}
    </div>
    <div class="c-foot">
      <span class="muted hide-sm"><kbd>Enter</kbd> to confirm, <kbd>Esc</kbd> to cancel</span>
      <span class="spacer"></span>
      <button class="btn" onclick={close} disabled={busy}>Cancel</button>
      <button bind:this={confirmEl} class="btn" class:pri={!ask.danger} class:danger={ask.danger} onclick={go} disabled={busy || !valid}>
        {#if busy}<span class="spinner"></span>{:else if ask.danger}<Icon name="stop" size={13} />{/if}{ask.confirm}
      </button>
    </div>
  </div>
</div>
