<script lang="ts">
  // Download is one click for what the view shows; the arrow picks the format or the whole range.
  import { app, load, save } from '../lib/state.svelte'
  import { api, apiFetch, query } from '../lib/api'
  import { hhmm } from '../lib/format'
  import Icon from './Icon.svelte'
  import Popover from './Popover.svelte'

  type Format = 'log' | 'jsonl' | 'csv'
  const FORMATS: [Format, string, string][] = [
    ['log', 'Plain text (.log)', 'traces kept whole'],
    ['jsonl', 'JSON Lines (.jsonl)', ''],
    ['csv', 'CSV (.csv)', ''],
  ]
  const L = app.logs
  let format = $state<Format>((load('swarmdash-dl') as Format | null) ?? 'log')
  let busy = $state(false)
  let open = $state(false)
  let arrow: HTMLButtonElement | undefined = $state()

  function pick(f: Format) {
    format = f
    save('swarmdash-dl', f)
  }

  async function download(all: boolean) {
    open = false
    busy = true
    const [from, to] = L.window()
    const p = all ? { from: Math.floor(from), to: Math.ceil(to) } : L.params()
    const path = `/c/${encodeURIComponent(app.activeId)}/logs/export`
    try {
      if (app.meta?.desktop) {
        const r = await api<{ path: string; entries: number }>(`${path}?${query({ ...p, format, save: 'true' })}`)
        app.notify(`Saved ${r.entries.toLocaleString()} entries to ${r.path}`)
      } else {
        const res = await apiFetch(`${path}?${query({ ...p, format })}`)
        const name = /filename="([^"]+)"/.exec(res.headers.get('content-disposition') ?? '')?.[1] ?? `swarmdash.${format}`
        const url = URL.createObjectURL(await res.blob())
        const a = document.createElement('a')
        a.href = url
        a.download = name
        a.click()
        setTimeout(() => URL.revokeObjectURL(url), 10000)
        app.notify(`Downloaded ${name}`)
      }
    } catch (e) {
      app.notify(app.describe(e), 'err')
    } finally {
      busy = false
    }
  }
</script>

<span class="split">
  <button class="ctl" onclick={() => download(false)} disabled={busy} title="Download what the view shows. Nothing is written to the server.">
    {#if busy}<span class="spinner" style="width:13px;height:13px;border-width:2px"></span>{:else}<Icon name="download" size={14} />{/if}Download .{format}
  </button>
  <button class="ctl" bind:this={arrow} aria-label="Download options" aria-haspopup="menu" aria-expanded={open} onclick={() => (open = !open)}><Icon name="chevD" size={13} /></button>
</span>
{#if open && arrow}
  {@const [from, to] = L.window()}
  <Popover anchor={arrow} align="right" label="Download options" onclose={() => (open = false)}>
    <div class="hd">Format</div>
    {#each FORMATS as [f, label, hint] (f)}
      <button class="it" class:sel={format === f} role="menuitemradio" aria-checked={format === f} onclick={() => pick(f)}>
        <span class="cb" class:on={format === f} style="border-radius:50%">{#if format === f}<Icon name="check" size={11} />{/if}</span>
        <span class="l">{label}</span><span class="r">{hint}</span>
      </button>
    {/each}
    <div class="sepl"></div>
    <div class="hd">Download now</div>
    <button class="it" role="menuitem" onclick={() => download(false)}><span class="l2"><span>What the view shows</span><small>Current filters and time range</small></span></button>
    <button class="it" role="menuitem" onclick={() => download(true)}><span class="l2"><span>Everything from {hhmm(from)} to {hhmm(to)}</span><small>All services, all levels</small></span></button>
    <div class="sepl"></div>
    <div class="note">Exports from the local store, with stack traces kept whole. Downloads go to this computer, never to the server.</div>
  </Popover>
{/if}
