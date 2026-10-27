<script lang="ts">
  // One entry of a log stream. Click expands it; the hover actions work without expanding.
  import { app } from '../lib/state.svelte'
  import { firstLine, hms, marks, tidy } from '../lib/format'
  import type { Entry } from '../lib/types'
  import Icon from './Icon.svelte'

  let { e, n = 1, open, ontoggle, terms = [], service = true, onservice, oncontext }: {
    e: Entry; n?: number; open: boolean; ontoggle: () => void; terms?: string[]; service?: boolean
    /** Narrows the stream to this entry's service. */
    onservice?: () => void
    /** Shows the lines around this entry. */
    oncontext?: () => void
  } = $props()

  const short = (id: string) => (id.length > 12 ? id.slice(0, 12) : id)
  let lines = $derived(e.text.split('\n'))

  function key(ev: KeyboardEvent) {
    if (ev.target !== ev.currentTarget) return
    if (ev.key === 'Enter' || ev.key === ' ') {
      ev.preventDefault()
      ontoggle()
    } else if (ev.key === 'c') {
      app.copy(e.text)
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="row {e.level}" class:open tabindex="0" role="button" aria-expanded={open}
  onclick={(ev) => { if (!(ev.target as HTMLElement).closest('button,.detail')) ontoggle() }} onkeydown={key}>
  <span class="ts">{hms(e.ts / 1000).slice(0, 12)}</span>
  <span class="lvl {e.level}">{e.level}</span>
  {#if service}
    <button class="svc" title="Show only {e.service}" onclick={() => onservice?.()}>{e.service}<span class="muted">.{e.replica}</span></button>
  {/if}
  <span class="msg">{#if !open}{#each marks(tidy(firstLine(e.text)), terms) as m, i (i)}{#if m.m}<mark>{m.t}</mark>{:else}{m.t}{/if}{/each}{#if e.lines > 1}<span class="rep" title="Multi-line entry, reassembled per replica">{e.lines} lines</span>{/if}{/if}{#if n > 1}<span class="rep">+{n - 1} similar</span>{/if}</span>
  <span class="ra">
    {#if service && onservice}<button class="ib" title="Only {e.service}" aria-label="Only {e.service}" onclick={() => onservice?.()}><Icon name="filter" size={13} /></button>{/if}
    {#if oncontext}<button class="ib" title="Show 30 s before and after" aria-label="Show 30 seconds around it" onclick={() => oncontext?.()}><Icon name="clock" size={13} /></button>{/if}
    <button class="ib" title="Copy (c)" aria-label="Copy" onclick={() => app.copy(e.text)}><Icon name="copy" size={13} /></button>
  </span>
  {#if open}
    <div class="detail">
      <pre class="trace" class:plain={e.level !== 'ERROR'}>{#each lines as l, i (i)}{#if i === 0}<b>{#each marks(l, terms) as m, j (j)}{#if m.m}<mark>{m.t}</mark>{:else}{m.t}{/if}{/each}</b>{:else}{'\n'}<span class="fr">{#each marks(l, terms) as m, j (j)}{#if m.m}<mark>{m.t}</mark>{:else}{m.t}{/if}{/each}</span>{/if}{/each}</pre>
      {#if e.raw}<details><summary class="muted" style="cursor:pointer;font-size:12px;font-family:var(--sans)">Original JSON line</summary><pre class="trace raw plain">{e.raw}</pre></details>{/if}
      <div class="kvl">
        <span>replica <b>{e.replica}</b></span><span>node <b>{e.node}</b></span><span>container <b>{short(e.source)}</b></span>
        <span>stream <b>{e.stream}</b></span><span>time <b>{new Date(e.ts / 1000).toISOString()}</b></span>
      </div>
    </div>
  {/if}
</div>
