<script lang="ts">
  import { app } from '../lib/state.svelte'
  import { del, post, put } from '../lib/api'
  import type { Connection, TestStep } from '../lib/types'

  type Kind = 'ssh' | 'local'
  let editing = $state<string | null>(null)
  let kind = $state<Kind>('ssh')
  let name = $state('')
  let target = $state('')
  let port = $state('')
  let identity = $state('')
  let jump = $state('')
  let socket = $state('/var/run/docker.sock')
  let endpoint = $state('')
  let saving = $state(false)
  let formError = $state<string | null>(null)
  let tests = $state<Record<string, TestStep[] | 'running'>>({})
  let confirmDelete = $state<string | null>(null)
  let formEl: HTMLElement | undefined = $state()

  function body() {
    if (kind === 'local') return { name: name.trim(), kind, endpoint: endpoint.trim() || null }
    return {
      name: name.trim() || target.trim(),
      kind,
      target: target.trim(),
      port: port.trim() ? Number(port) : null,
      identity: identity.trim() || null,
      jump: jump.trim() || null,
      socket: socket.trim() || '/var/run/docker.sock',
    }
  }

  function reset() {
    editing = null
    kind = 'ssh'
    name = target = port = identity = jump = endpoint = ''
    socket = '/var/run/docker.sock'
    formError = null
    delete tests['new']
  }

  function edit(c: Connection) {
    editing = c.id
    kind = c.kind
    name = c.name
    formError = null
    if (c.kind === 'ssh') {
      target = c.target
      port = c.port ? String(c.port) : ''
      identity = c.identity ?? ''
      jump = c.jump ?? ''
      socket = c.socket
    } else {
      endpoint = c.endpoint ?? ''
    }
    formEl?.scrollIntoView({ behavior: 'smooth', block: 'start' })
  }

  async function save(e: SubmitEvent) {
    e.preventDefault()
    saving = true
    formError = null
    try {
      if (editing) {
        await put<Connection>(`/connections/${encodeURIComponent(editing)}`, body())
        app.notify(`Saved ${name}.`)
      } else {
        const c = await post<Connection>('/connections', body())
        await app.reloadConnections()
        app.select(c.id)
        app.notify(`Added ${c.name}. It is now the active connection.`)
      }
      await app.reloadConnections()
      reset()
    } catch (err) {
      formError = app.describe(err)
    } finally {
      saving = false
    }
  }

  async function test(key: string, input: object) {
    tests[key] = 'running'
    try {
      tests[key] = await post<TestStep[]>('/connections/test', input)
    } catch (err) {
      tests[key] = [{ label: app.describe(err), ok: false, detail: null, ms: 0 }]
    }
  }

  function inputOf(c: Connection) {
    const { id: _, ...rest } = c
    return rest
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

  function targetOf(c: Connection) {
    if (c.kind === 'local') return c.endpoint || 'Default Docker socket'
    return `${c.target}${c.port ? ':' + c.port : ''}`
  }

  function open(c: Connection) {
    app.select(c.id)
    app.go('overview')
  }
</script>

<div class="conns">
  {#each app.connections as c (c.id)}
    {@const active = c.id === app.activeId}
    {@const t = tests[c.id]}
    <div class="panel cc">
      <div class="h">
        <span class="dot {active ? (app.conn?.status === 'connected' ? '' : app.conn?.status === 'error' ? 'err' : 'off') : 'off'}"></span>
        <b>{c.name}</b>
        <span class="pill {c.kind === 'ssh' ? 'acc' : ''}">{c.kind === 'ssh' ? 'SSH' : 'Local'}</span>
        <span class="spacer"></span>
        {#if active}<span class="pill ok">active</span>{/if}
      </div>
      <dl class="kv">
        <dt>Target</dt><dd>{targetOf(c)}</dd>
        {#if c.kind === 'ssh'}
          <dt>Docker socket</dt><dd>{c.socket}</dd>
          <dt>Key</dt><dd>{c.identity || 'Your SSH setup (agent, ~/.ssh/config)'}</dd>
          {#if c.jump}<dt>Jump host</dt><dd>{c.jump}</dd>{/if}
        {/if}
      </dl>
      <div style="display:flex;gap:8px;flex-wrap:wrap;align-items:center">
        <button class="btn pri" disabled={active} onclick={() => open(c)}>Open dashboard</button>
        <button class="btn ghost" onclick={() => test(c.id, inputOf(c))} disabled={t === 'running'}>Test</button>
        <button class="btn ghost" onclick={() => edit(c)}>Edit</button>
        {#if confirmDelete === c.id}
          <span class="confirm">Remove it and its stored logs?
            <button class="btn danger" onclick={() => remove(c)}>Remove</button>
            <button class="btn ghost" onclick={() => (confirmDelete = null)}>Keep</button></span>
        {:else}
          <button class="btn ghost" onclick={() => (confirmDelete = c.id)}>Remove</button>
        {/if}
      </div>
      {#if t}{@render steps(t)}{/if}
    </div>
  {/each}
</div>

<section class="panel" bind:this={formEl}>
  <div class="ph"><h2>{editing ? `Edit ${name || 'connection'}` : 'Add a connection'}</h2>{#if editing}<span class="spacer"></span><button class="btn ghost" onclick={reset}>Cancel</button>{/if}</div>
  <form class="pb form" onsubmit={save}>
    <div class="full">
      <span style="font-size:12px;color:var(--muted);font-weight:500">Type</span>
      <div class="seg" role="group" style="margin-top:6px">
        <button type="button" aria-pressed={kind === 'ssh'} onclick={() => (kind = 'ssh')}>SSH to a server</button>
        <button type="button" aria-pressed={kind === 'local'} onclick={() => (kind = 'local')}>This computer</button>
      </div>
    </div>
    <label>Name<input bind:value={name} placeholder={kind === 'ssh' ? 'e.g. prod-server' : 'This computer'} autocomplete="off" /></label>
    {#if kind === 'ssh'}
      <label>Host<input bind:value={target} placeholder="user@host or an alias from ~/.ssh/config" autocomplete="off" spellcheck="false" required /></label>
      <label>Port <span class="muted">(optional)</span><input bind:value={port} inputmode="numeric" placeholder="22" autocomplete="off" /></label>
      <label>Key file <span class="muted">(optional)</span><input bind:value={identity} placeholder="~/.ssh/id_ed25519" autocomplete="off" spellcheck="false" /></label>
      <label>Jump host <span class="muted">(optional)</span><input bind:value={jump} placeholder="user@bastion:22" autocomplete="off" spellcheck="false" /></label>
      <label>Docker socket on the server<input bind:value={socket} autocomplete="off" spellcheck="false" /></label>
      <div class="full hint">swarmdash runs your own <span class="mono">ssh</span>, so your keys, agent and <span class="mono">~/.ssh/config</span> (aliases, ports, ProxyJump) work as in a terminal. It forwards the server's Docker socket through that one connection and only ever reads from it. Keys are never copied or stored. The SSH user needs read access to the socket, which usually means being in the <span class="mono">docker</span> group.</div>
    {:else}
      <label class="full">Docker endpoint <span class="muted">(optional)</span><input bind:value={endpoint} placeholder="unix:///var/run/docker.sock" autocomplete="off" spellcheck="false" /></label>
      <div class="full hint">Leave it empty to use Docker's default socket, or <span class="mono">DOCKER_HOST</span> when it is set.</div>
    {/if}
    <div class="full" style="display:flex;gap:8px;flex-wrap:wrap;align-items:center">
      <button class="btn" type="button" onclick={() => test('new', body())} disabled={tests['new'] === 'running'}>Test connection</button>
      <button class="btn pri" type="submit" disabled={saving}>{editing ? 'Save changes' : 'Save connection'}</button>
    </div>
    {#if formError}<div class="full err-banner">{formError}</div>{/if}
    {#if tests['new']}<div class="full">{@render steps(tests['new'])}</div>{/if}
  </form>
</section>

{#snippet steps(t: TestStep[] | 'running')}
  <div class="steps">
    {#if t === 'running'}
      <div><span class="spinner"></span>Testing…</div>
    {:else}
      {#each t as s, i (i)}
        <div style="align-items:flex-start">
          <span class="dot {s.ok ? '' : 'err'}" style="margin-top:6px"></span>
          <span>{s.label}{#if s.ms} <span class="muted mono" style="font-size:11px">{s.ms} ms</span>{/if}
            {#if s.detail}<br /><span class="muted" style="white-space:pre-wrap;font-size:12.5px">{s.detail}</span>{/if}</span>
        </div>
      {/each}
    {/if}
  </div>
{/snippet}
