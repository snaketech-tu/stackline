<script lang="ts">
  import { app } from '../lib/state.svelte'
  import { api, apiFetch, query } from '../lib/api'
  import { hhmm } from '../lib/format'
  import Drawer from './Drawer.svelte'

  let format = $state<'log' | 'jsonl' | 'csv'>('log')
  let scope = $state<'filters' | 'all'>('filters')
  let busy = $state(false)
  let msg = $state<string | null>(null)

  const L = app.logs
  let [from, to] = L.window()

  function params() {
    const p = scope === 'filters' ? L.params() : { from: Math.floor(from), to: Math.ceil(to) }
    return { ...p, format }
  }

  async function download(e: SubmitEvent) {
    e.preventDefault()
    busy = true
    msg = null
    const path = `/c/${encodeURIComponent(app.activeId)}/logs/export`
    try {
      if (app.meta?.desktop) {
        const r = await api<{ path: string; entries: number }>(`${path}?${query({ ...params(), save: 'true' })}`)
        msg = `Saved ${r.entries.toLocaleString()} entries to ${r.path}`
      } else {
        const res = await apiFetch(`${path}?${query(params())}`)
        const name = /filename="([^"]+)"/.exec(res.headers.get('content-disposition') ?? '')?.[1] ?? `swarmdash.${format}`
        const url = URL.createObjectURL(await res.blob())
        const a = document.createElement('a')
        a.href = url
        a.download = name
        a.click()
        setTimeout(() => URL.revokeObjectURL(url), 10000)
        msg = `Downloaded ${name}`
      }
    } catch (err) {
      msg = app.describe(err)
    } finally {
      busy = false
    }
  }
</script>

<Drawer title="Download logs">
  <span class="hint">Exports from the local store, with multi-line stack traces kept whole. Downloading is the only thing swarmdash writes, and it writes to this computer, never to the server.</span>
  <form class="form" style="grid-template-columns:minmax(0,1fr)" onsubmit={download}>
    <label>Format
      <select bind:value={format}>
        <option value="log">Plain text (.log), one entry per block</option>
        <option value="jsonl">JSON Lines (.jsonl)</option>
        <option value="csv">CSV (.csv)</option>
      </select>
    </label>
    <label>What to include
      <select bind:value={scope}>
        <option value="filters">What the logs view shows now</option>
        <option value="all">Everything from {hhmm(from)} to {hhmm(to)}, all services and levels</option>
      </select>
    </label>
    <div style="display:flex;gap:8px;align-items:center">
      <button class="btn pri" type="submit" disabled={busy}>{busy ? 'Preparing…' : 'Download'}</button>
      {#if busy}<span class="spinner"></span>{/if}
    </div>
    {#if msg}<div class="hint" style="word-break:break-all">{msg}</div>{/if}
  </form>
</Drawer>
