<script lang="ts">
  import { app } from '../lib/state.svelte'
  import { del, post } from '../lib/api'
  import type { Connection, TestStep } from '../lib/types'
  import ConnectionForm from '../components/ConnectionForm.svelte'
  import Icon from '../components/Icon.svelte'
  import TestSteps from '../components/TestSteps.svelte'

  let editing = $state<string | null>(null)
  let confirmDelete = $state<string | null>(null)
  let tests = $state<Record<string, TestStep[] | 'running'>>({})

  function targetOf(c: Connection) {
    if (c.kind === 'local') return c.endpoint || 'Default Docker socket'
    return `${c.target}${c.port ? ':' + c.port : ''}`
  }
  function dot(c: Connection) {
    if (c.id !== app.activeId) return 'off'
    return app.conn?.status === 'connected' ? '' : app.conn?.status === 'error' ? 'err' : 'off'
  }
  async function test(c: Connection) {
    const { id: _, ...input } = c
    tests[c.id] = 'running'
    try {
      tests[c.id] = await post<TestStep[]>('/connections/test', input)
    } catch (err) {
      tests[c.id] = [{ label: app.describe(err), ok: false, detail: null, ms: 0 }]
    }
  }
  async function remove(c: Connection) {
    try {
      await del(`/connections/${encodeURIComponent(c.id)}`)
      confirmDelete = null
      await app.reloadConnections()
      app.notify(`Removed ${c.name} and its stored logs.`)
    } catch (err) {
      app.notify(app.describe(err), 'err')
    }
  }
  function open(c: Connection) {
    app.select(c.id)
    app.go('overview')
  }
</script>

<section class="panel">
  <div class="ph"><h2>Connections</h2><span class="spacer"></span><button class="btn pri sm" onclick={() => { editing = null; app.drawer = { kind: 'connection', id: null } }}><Icon name="plus" size={14} />Add connection</button></div>
  <div class="tw">
    <table>
      <thead><tr><th>Name</th><th>Target</th><th class="hide-sm">Kind</th><th class="hide-sm">Latency</th><th class="actcell"><span class="sr">Actions</span></th></tr></thead>
      <tbody>
        {#each app.connections as c (c.id)}
          {@const active = c.id === app.activeId}
          <tr class="click" onclick={() => open(c)} title="Open {c.name}">
            <td><span style="display:inline-flex;gap:10px;align-items:center"><span class="dot {dot(c)}"></span><b>{c.name}</b>{#if active}<span class="pill ok">active</span>{/if}</span></td>
            <td class="mono">{targetOf(c)}{#if c.kind === 'ssh' && c.jump}<span class="muted"> via {c.jump}</span>{/if}</td>
            <td class="hide-sm"><span class="pill {c.kind === 'ssh' ? 'acc' : ''}">{c.kind === 'ssh' ? 'SSH' : 'Local'}</span></td>
            <td class="hide-sm mono muted">{active && app.conn?.latencyMs != null ? `${app.conn.latencyMs} ms` : '–'}</td>
            <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
            <td class="actcell" onclick={(e) => e.stopPropagation()}>
              {#if confirmDelete === c.id}
                <span class="confirm">Remove it and its stored logs?
                  <button class="btn sm danger" onclick={() => remove(c)}>Remove</button>
                  <button class="btn sm ghost" onclick={() => (confirmDelete = null)}>Keep</button></span>
              {:else}
                <span class="acts" style:opacity={editing === c.id || tests[c.id] ? 1 : undefined}>
                  <button class="btn ghost sm" onclick={() => test(c)} disabled={tests[c.id] === 'running'}>Test</button>
                  <button class="btn ghost sm" onclick={() => (editing = editing === c.id ? null : c.id)}>Edit</button>
                  <button class="btn ghost sm" onclick={() => (confirmDelete = c.id)}>Remove</button>
                </span>
              {/if}
            </td>
          </tr>
          {#if tests[c.id] && editing !== c.id}
            <tr class="editrow"><td colspan="5"><div style="display:flex;gap:12px;align-items:flex-start"><TestSteps steps={tests[c.id]} /><span class="spacer"></span><button class="ib" aria-label="Hide test result" onclick={() => delete tests[c.id]}><Icon name="x" size={14} /></button></div></td></tr>
          {/if}
          {#if editing === c.id}
            <tr class="editrow"><td colspan="5">{#key c.id}<ConnectionForm conn={c} ondone={() => (editing = null)} />{/key}</td></tr>
          {/if}
        {:else}
          <tr><td colspan="5" class="empty">No connections yet. Add the server you want to watch.</td></tr>
        {/each}
      </tbody>
    </table>
  </div>
</section>
<div class="hint" style="padding:0 4px">Click a connection to open its overview. Stackline keeps pins and stored logs per connection.</div>
