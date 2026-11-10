<script lang="ts" module>
  export interface Col<T> {
    id: string
    label: string
    /** Value to sort by; columns without one cannot be sorted. */
    sort?: (r: T) => string | number | null
    /** Class for the header cell, such as `hide-sm` or `num`. */
    cls?: string
    title?: string
  }
  export interface Act<T> {
    /** The key that runs it while the list has the cursor. Shown on the button. */
    key: string
    label: string
    icon?: string
    run: (r: T) => void
    /** Hidden for rows it does not fit. */
    when?: (r: T) => boolean
    /** Leaves it out of the action bar; the key still works. */
    hidden?: boolean
  }
  export interface Tab<T> {
    id: string
    label: string
    match: (r: T) => boolean
  }
</script>

<script lang="ts" generics="T">
  // A list the way Kubri shows one: a filter, tabs, an action bar that names
  // the row it acts on, and a cursor that moves with j/k or the arrow keys.
  // Enter opens what is inside the row; a click opens its inspector.
  import { tick, type Snippet } from 'svelte'
  import { app } from '../lib/state.svelte'
  import { compare, type ListState, type ListTab } from '../lib/lists.svelte'
  import Icon from './Icon.svelte'

  let {
    list, items, key, label, cols, row, tabs, match, actions = [], onopen, openLabel = 'Open', oninspect, inspecting,
    title, noun, scope, onscopeclear, toolbar, before, empty = 'Nothing to show.', rowClass,
  }: {
    list: ListState
    items: T[]
    key: (r: T) => string
    /** How the action bar names a row. */
    label: (r: T) => string
    cols: Col<T>[]
    row: Snippet<[T]>
    tabs: Tab<T>[]
    /** Whether a row matches the typed filter (lower case). */
    match: (r: T, q: string) => boolean
    actions?: Act<T>[]
    /** Enter: drill into the row. */
    onopen?: (r: T) => void
    openLabel?: string
    /** A click, or `d`: show the row in the inspector. */
    oninspect?: (r: T) => void
    /** The key of the row the inspector shows, so moving the cursor takes the inspector along. */
    inspecting?: string | null
    title: string
    noun: [string, string]
    /** Says what the list is narrowed to, such as "of stack shop", with a way back to all. */
    scope?: string
    onscopeclear?: () => void
    toolbar?: Snippet
    before?: Snippet
    empty?: string
    rowClass?: (r: T) => string
  } = $props()

  let filterEl: HTMLInputElement | undefined = $state()
  let bodyEl: HTMLElement | undefined = $state()

  let needle = $derived(list.q.trim().toLowerCase())
  let matching = $derived(needle ? items.filter((r) => match(r, needle)) : items)
  let counts = $derived(Object.fromEntries(tabs.map((t) => [t.id, matching.filter(t.match).length])))
  let tab = $derived(tabs.find((t) => t.id === list.tab) ?? tabs[0])
  let shown = $derived.by(() => {
    const out = matching.filter(tab.match)
    const c = list.sort && cols.find((x) => x.id === list.sort!.col)
    if (!c?.sort) return out
    const dir = list.sort!.desc ? -1 : 1
    return out.map((r, i) => ({ r, i, v: c.sort!(r) })).sort((a, b) => dir * compare(a.v, b.v) || a.i - b.i).map((x) => x.r)
  })
  let cur = $derived(list.cursor ? shown.find((r) => key(r) === list.cursor) : undefined)
  let barActs = $derived(actions.filter((a) => !a.hidden))

  // A cursor the filter or tab no longer shows is dropped, so nothing acts on a row out of sight.
  $effect(() => {
    if (list.cursor && !shown.some((r) => key(r) === list.cursor)) list.cursor = null
  })

  async function reveal() {
    await tick()
    bodyEl?.querySelector<HTMLElement>('tr.cur')?.scrollIntoView({ block: 'nearest' })
  }

  function moveTo(r: T | undefined) {
    if (!r) return
    list.cursor = key(r)
    // The inspector follows the cursor while it is open on this list.
    if (oninspect && inspecting && inspecting !== key(r)) oninspect(r)
    reveal()
  }

  function step(d: number) {
    if (!shown.length) return
    const i = cur ? shown.indexOf(cur) : -1
    moveTo(shown[i < 0 ? (d > 0 ? 0 : shown.length - 1) : Math.max(0, Math.min(shown.length - 1, i + d))])
  }

  function click(r: T) {
    list.cursor = key(r)
    oninspect?.(r)
  }

  function keydown(e: KeyboardEvent) {
    if (app.palette || app.help || app.ask || e.ctrlKey || e.metaKey || e.altKey || app.awaitingGo()) return
    const el = document.activeElement as HTMLElement | null
    const tag = el?.tagName
    if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return
    // Keys typed in the inspector or the workspace belong to them.
    if (el?.closest('.drawer, .dock')) return
    const k = e.key
    // Enter on a focused button or link is that button's.
    if ((tag === 'BUTTON' || tag === 'A') && (k === 'Enter' || k === ' ')) return
    if (k === 'j' || k === 'ArrowDown') step(1)
    else if (k === 'k' || k === 'ArrowUp') step(-1)
    else if (k === 'Home') moveTo(shown[0])
    else if (k === 'End') moveTo(shown.at(-1))
    else if (k === 'PageDown') step(10)
    else if (k === 'PageUp') step(-10)
    else if (k === '/') filterEl?.focus()
    else if (k === 'Enter' && cur && onopen) onopen(cur)
    else if (k === 'd' && cur && oninspect) oninspect(cur)
    else {
      const a = cur && actions.find((x) => x.key === k && (!x.when || x.when(cur!)))
      if (!a || !cur) return
      a.run(cur)
    }
    e.preventDefault()
  }

  function filterKey(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.stopPropagation()
      if (list.q) list.q = ''
      else filterEl?.blur()
    } else if (e.key === 'Enter' || e.key === 'ArrowDown') {
      // Straight from the filter to the first match; Enter does not also open it.
      e.preventDefault()
      e.stopPropagation()
      filterEl?.blur()
      moveTo(shown[0])
    }
  }

  function setTab(id: string) {
    list.tab = id as ListTab
  }
</script>

<svelte:window onkeydown={keydown} />

<div class="phead">
  <nav class="crumbs" aria-label="Where you are">
    <span>{app.active?.name ?? '…'}</span>
    {#if app.scope}<Icon name="chev" size={12} /><span>{app.scope}</span>{/if}
    <Icon name="chev" size={12} /><span>{title}</span>
  </nav>
  <div class="ptitle">
    <h2>{title}</h2><span class="pcount">{shown.length}</span>
    {#if scope}
      <button class="fchip scopechip" onclick={onscopeclear} title="Show all {noun[1]} again"><span class="t">{scope}</span><span class="x"><Icon name="x" size={11} /></span></button>
    {/if}
    <span class="spacer"></span>
    {@render toolbar?.()}
  </div>
</div>

{@render before?.()}

<section class="panel rlist">
  <div class="rl-filter">
    <label class="search">
      <Icon name="search" size={14} />
      <input bind:this={filterEl} bind:value={list.q} onkeydown={filterKey} placeholder="Filter by name, image, stack or node" autocomplete="off" spellcheck="false" aria-label="Filter {noun[1]}" />
      {#if list.q}<button class="ib" style="width:22px;height:22px" onclick={() => (list.q = '')} aria-label="Clear filter"><Icon name="x" size={12} /></button>{:else}<kbd>/</kbd>{/if}
    </label>
  </div>
  <div class="rl-tabs" role="tablist" aria-label="Show">
    {#each tabs as t (t.id)}
      <button role="tab" aria-selected={tab.id === t.id} onclick={() => setTab(t.id)}>{t.label}<span class="n" class:bad={t.id === 'attention' && counts[t.id]}>{counts[t.id]}</span></button>
    {/each}
  </div>
  <div class="rl-actions" aria-label="Actions">
    {#if cur}
      <span class="target mono" title={label(cur)}>{label(cur)}</span>
    {:else}
      <span class="target none">Pick a row with <kbd>j</kbd> <kbd>k</kbd> or a click</span>
    {/if}
    {#if onopen}<button class="btn sm" disabled={!cur} onclick={() => cur && onopen(cur)}>{openLabel}<kbd>↵</kbd></button>{/if}
    {#if oninspect}<button class="btn sm" disabled={!cur} onclick={() => cur && oninspect(cur)}>Inspect<kbd>d</kbd></button>{/if}
    {#each barActs as a (a.key)}
      {@const ok = !!cur && (!a.when || a.when(cur))}
      <button class="btn sm" disabled={!ok} onclick={() => cur && a.run(cur)}>{#if a.icon}<Icon name={a.icon} size={13} />{/if}{a.label}<kbd>{a.key}</kbd></button>
    {/each}
  </div>
  <div class="tw" bind:this={bodyEl}>
    <table>
      <thead>
        <tr>
          {#each cols as c (c.id)}
            <th class={c.cls} title={c.title} aria-sort={list.sort?.col === c.id ? (list.sort.desc ? 'descending' : 'ascending') : undefined}>
              {#if c.sort}
                <button class="sorter" onclick={() => list.sortBy(c.id)}>{c.label}<span class="arrow" aria-hidden="true">{list.sort?.col === c.id ? (list.sort.desc ? '↓' : '↑') : '↕'}</span></button>
              {:else}{c.label}{/if}
            </th>
          {/each}
        </tr>
      </thead>
      <tbody>
        {#each shown as r (key(r))}
          <tr class="click {rowClass?.(r) ?? ''}" class:cur={cur === r} class:inspected={inspecting === key(r)} data-key={key(r)}
            onclick={() => click(r)} ondblclick={() => onopen?.(r)} aria-selected={cur === r}>
            {@render row(r)}
          </tr>
        {:else}
          <tr><td colspan={cols.length} class="empty">
            {needle || tab.id !== tabs[0].id ? `No ${noun[1]} match.` : empty}
            {#if needle || tab.id !== tabs[0].id}<button class="btn ghost sm" onclick={() => { list.q = ''; list.tab = tabs[0].id as ListTab }}>Show all</button>{/if}
          </td></tr>
        {/each}
      </tbody>
    </table>
  </div>
  <div class="rl-foot">
    <span>{shown.length === items.length ? `${items.length} ${items.length === 1 ? noun[0] : noun[1]}` : `${shown.length} of ${items.length} ${noun[1]}`}</span>
    <span class="spacer"></span>
    {#if app.access.writable}<span class="hide-sm warn-t">Changes allowed: each one asks first</span>
    {:else}<span class="hide-sm">Read-only: nothing here changes your containers</span>{/if}
  </div>
</section>
