<script lang="ts">
  // One tab of the log workspace: the newest lines of its source, followed
  // live, with levels and a search of its own. Older lines load on scroll.
  import { untrack } from 'svelte'
  import { app, LEVELS, LogFilters } from '../lib/state.svelte'
  import { fetchLogs, type LogParams } from '../lib/api'
  import { LINES, LogPages, nearEnd } from '../lib/pages.svelte'
  import { searchTerms } from '../lib/format'
  import type { DockTab } from '../lib/dock.svelte'
  import type { Level } from '../lib/types'
  import Icon from './Icon.svelte'
  import LogRow from './LogRow.svelte'

  let { tab }: { tab: DockTab } = $props()
  const COL: Record<Level, string> = { ERROR: 'var(--err)', WARN: 'var(--warn)', INFO: 'var(--info)', DEBUG: 'var(--dbg)' }
  const F = new LogFilters()
  const pages = new LogPages()
  let qInput = $state('')
  let q = $state('')
  let paused = $state(false)
  let error = $state<string | null>(null)
  $effect(() => {
    const v = qInput
    const t = setTimeout(() => (q = v.trim()), 250)
    return () => clearTimeout(t)
  })

  let stack = $derived(tab.source.startsWith('stack:') ? tab.source.slice(6) : '')
  let service = $derived(tab.source && !stack ? tab.source : '')
  /** The query as it stands; scrolling down pages through it. */
  let params: LogParams | null = null
  let seq = 0

  $effect(() => {
    const id = app.activeId
    const lv = [...F.levels].join(',')
    const filter = [tab.q, q].filter(Boolean).join(' ')
    const [st, sv] = [stack, service]
    const mine = ++seq
    untrack(() => pages.reset())
    const make = (): LogParams => {
      const days = untrack(() => app.settings?.retentionDays) ?? 2
      const p: LogParams = { ...app.storedRange(days * 86400e3), open: true, levels: lv, buckets: 1 }
      if (filter) p.q = filter
      if (st) p.stack = st
      if (sv) p.service = sv
      return p
    }
    const get = () => {
      const p = make()
      params = p
      const hp = pages.headParams(p)
      fetchLogs(id, hp)
        .then((r) => {
          if (mine !== seq) return
          pages.takeHead(r, hp)
          error = null
        })
        .catch((e) => {
          if (mine === seq) error = app.describe(e)
        })
    }
    untrack(get)
    const every = Math.max(1, untrack(() => app.settings?.logPollSecs) ?? 3) * 1000
    const t = setInterval(() => !document.hidden && !untrack(() => paused) && get(), every)
    return () => clearInterval(t)
  })
  const older = () => (params ? pages.loadOlder(app.activeId, params, app.describe) : false)
  let terms = $derived(searchTerms([tab.q, q].join(' ')))

  function openFull() {
    app.showLogs(tab.source ? [tab.source] : [])
    app.logs.q = [tab.q, q].filter(Boolean).join(' ')
  }
</script>

<div class="dl-bar">
  <span class="lvgroup" role="group" aria-label="Levels">
    {#each LEVELS as l (l)}
      <button class="lv sm" aria-pressed={F.levels.has(l)} onclick={(e) => F.pickLevel(l, e.shiftKey || e.metaKey || e.ctrlKey)} title="Click: only {l.toLowerCase()}. Shift-click: add or remove.">
        <span class="sw" style:background={COL[l]}></span>{l[0] + l.slice(1).toLowerCase()}<span class="n">{pages.head?.result.levels[l] ?? 0}</span>
      </button>
    {/each}
  </span>
  <label class="search">
    <Icon name="search" size={13} />
    <input bind:value={qInput} placeholder="Search these lines" autocomplete="off" spellcheck="false" aria-label="Search {tab.label}" />
    {#if qInput}<button class="ib" style="width:20px;height:20px" onclick={() => (qInput = '')} aria-label="Clear search"><Icon name="x" size={11} /></button>{/if}
  </label>
  {#if tab.q}<span class="fchip static"><span class="t">{tab.q}</span></span>{/if}
  <button class="ctl sm" class:live={!paused} aria-pressed={!paused} onclick={() => (paused = !paused)} title={paused ? 'Follow new lines again' : 'Stop following new lines'}><span class="live-dot"></span>{paused ? 'Paused' : 'Live'}</button>
  <button class="ctl sm" onclick={openFull} title="Open these lines on the Logs page, with the graph, error groups and download"><Icon name="external" size={13} /><span class="hide-sm">Open in Logs</span></button>
</div>
{#if error}<div class="err-banner" style="margin:8px">{error}</div>{/if}
<div class="dl-body">
  <div class="stream" class:nosvc={!!service}>
    {#each pages.entries as e (e.id)}
      <LogRow {e} service={!service} {terms} open={F.openRows.has(e.id)} ontoggle={() => F.toggleRow(e.id)} />
    {:else}
      <div class="empty">{pages.head ? 'No lines stored that match.' : 'Reading lines…'}</div>
    {/each}
    {#if pages.head && pages.entries.length}
      <div class="stream-end" use:nearEnd={older}>
        {#if pages.olderError}
          <span class="err-text">Older lines could not be read: {pages.olderError}</span>
        {:else if pages.more && !pages.full}
          <button class="btn sm" onclick={older} disabled={pages.loadingOlder}>Show {LINES.step.toLocaleString()} older lines</button>
          <span class="muted">{pages.entries.length.toLocaleString()} of {pages.head.result.total.toLocaleString()} shown</span>
        {:else if pages.more}
          <span>The workspace shows at most {LINES.max.toLocaleString()} lines. Open in Logs for older ones.</span>
        {:else}
          <span class="muted">These are all the stored lines that match.</span>
        {/if}
      </div>
    {/if}
  </div>
</div>
