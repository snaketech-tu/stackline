<script lang="ts">
  // Ctrl K: jump to a service, stack, error group, page or connection.
  import { onMount } from 'svelte'
  import { app, VIEWS } from '../lib/state.svelte'
  import { api, query } from '../lib/api'
  import { firstLine } from '../lib/format'
  import type { Group, LogResponse } from '../lib/types'
  import Icon from './Icon.svelte'

  type Item = { kind: string; label: string; right: string; text: string; run: () => void; logs?: () => void; html?: boolean }

  let q = $state('')
  let sel = $state(0)
  let input: HTMLInputElement | undefined = $state()
  let list: HTMLDivElement | undefined = $state()
  let groups = $state<Group[]>([])

  onMount(() => {
    input?.focus()
    const to = Date.now()
    api<LogResponse>(`/c/${encodeURIComponent(app.activeId)}/logs?${query({ from: to - 3600e3, to, levels: 'ERROR', buckets: 1, limit: 1 })}`)
      .then((r) => (groups = r.result.groups))
      .catch(() => {})
  })

  function close() {
    app.palette = false
  }

  let all = $derived.by(() => {
    const out: Item[] = []
    for (const s of app.services) {
      const e = app.errors?.byService[s.fullName] ?? 0
      out.push({
        kind: 'Service', label: s.fullName, text: `${s.fullName} ${s.image}`,
        right: `${s.running}/${s.desired}${e ? ` · ${e} errors` : ''}${app.isPinned(s.key) ? ' · ★' : ''}`,
        run: () => app.openService(s.key), logs: () => app.showLogs([s.fullName]),
      })
    }
    for (const st of app.snap?.stacks ?? []) {
      out.push({
        kind: 'Stack', label: st.name, text: `${st.name} stack`, right: `${st.services.length} services`,
        run: () => { app.stackQ = st.name; app.stackFilter = 'all'; app.go('stacks') }, logs: () => app.showLogs([`stack:${st.name}`]),
      })
    }
    for (const g of groups) {
      const p = g.pattern || firstLine(g.sample)
      out.push({ kind: 'Error', label: p, text: `${p} ${g.services.join(' ')} error`, right: `${g.count}× · ${g.services.join(', ')}`, run: () => app.showGroup(g.fp) })
    }
    for (const [v, label] of VIEWS) out.push({ kind: 'Page', label, text: `${label} ${v}`, right: '', run: () => app.go(v) })
    for (const c of app.connections) {
      if (c.id !== app.activeId) out.push({ kind: 'Connect', label: `Switch to ${c.name}`, text: `connection switch ${c.name}`, right: c.kind === 'ssh' ? 'SSH' : 'Local', run: () => app.select(c.id) })
    }
    out.push({ kind: 'Action', label: 'Show errors of the last hour', text: 'errors only last hour', right: '', run: () => { app.showLogs([], ['ERROR']); app.logs.setPreset('1h') } })
    out.push({ kind: 'Action', label: 'Add a connection', text: 'add connection server ssh', right: '', run: () => { app.go('connections'); app.drawer = { kind: 'connection', id: null } } })
    out.push({ kind: 'Action', label: 'Change theme', text: 'theme dark light system', right: app.theme, run: () => app.cycleTheme() })
    return out
  })

  let items = $derived.by(() => {
    const words = q.toLowerCase().split(/\s+/).filter(Boolean)
    if (!words.length) {
      const top = all.filter((x) => x.kind === 'Service' && (x.right.includes('★') || /^0\/[1-9]/.test(x.right)))
      return [...top, ...all.filter((x) => x.kind === 'Error').slice(0, 3), ...all.filter((x) => x.kind === 'Page')]
    }
    return all.filter((x) => words.every((w) => x.text.toLowerCase().includes(w))).slice(0, 30)
  })
  $effect(() => {
    void q
    sel = 0
  })

  function run(i: number, logs = false) {
    const it = items[i]
    if (!it) return
    close()
    ;(logs && it.logs ? it.logs : it.run)()
  }

  function key(e: KeyboardEvent) {
    if (e.key === 'ArrowDown' || (e.key === 'n' && e.ctrlKey)) sel = Math.min(items.length - 1, sel + 1)
    else if (e.key === 'ArrowUp' || (e.key === 'p' && e.ctrlKey)) sel = Math.max(0, sel - 1)
    else if (e.key === 'Enter') run(sel, e.shiftKey)
    else if (e.key === 'Escape') close()
    else return
    e.preventDefault()
    e.stopPropagation()
    list?.querySelector<HTMLElement>(`[data-i="${sel}"]`)?.scrollIntoView({ block: 'nearest' })
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="pal-wrap" onclick={(e) => { if (e.target === e.currentTarget) close() }}>
  <div class="pal" role="dialog" aria-modal="true" aria-label="Jump to">
    <div class="in">
      <Icon name="search" size={16} />
      <input bind:this={input} bind:value={q} onkeydown={key} placeholder="Service, stack, error, page or connection…" autocomplete="off" spellcheck="false"
        role="combobox" aria-expanded="true" aria-controls="pal-list" aria-activedescendant="pal-{sel}" />
      <kbd>Esc</kbd>
    </div>
    <div class="list" id="pal-list" role="listbox" bind:this={list}>
      {#if !q}<div class="hd">Pinned, failing and top errors</div>{/if}
      {#each items as it, i (it.kind + it.label)}
        <button class="it" class:sel={i === sel} id="pal-{i}" data-i={i} role="option" aria-selected={i === sel} onmousemove={() => (sel = i)} onclick={(e) => run(i, e.shiftKey)}>
          <span class="k">{it.kind}</span>
          <span class="l" class:mono={it.kind === 'Error'} style:font-weight={it.kind === 'Service' || it.kind === 'Stack' ? 600 : undefined}>{it.label}</span>
          <span class="r">{it.right}</span>
        </button>
      {:else}
        <div class="empty">Nothing matches.</div>
      {/each}
    </div>
    <div class="ft"><span><kbd>↑</kbd> <kbd>↓</kbd> move</span><span><kbd>Enter</kbd> open</span><span><kbd>Shift</kbd> <kbd>Enter</kbd> open its logs</span></div>
  </div>
</div>
