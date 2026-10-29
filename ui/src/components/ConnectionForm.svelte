<script lang="ts">
  // Add or edit a connection. Saving tests it first and shows each step.
  import { untrack } from 'svelte'
  import { app } from '../lib/state.svelte'
  import { post, put } from '../lib/api'
  import type { Connection, TestStep } from '../lib/types'
  import Icon from './Icon.svelte'
  import TestSteps from './TestSteps.svelte'

  let { conn = null, ondone }: { conn?: Connection | null; ondone: () => void } = $props()

  type Kind = 'ssh' | 'local'
  /** Where the engines listen. Podman needs its API socket switched on: `systemctl [--user] enable --now podman.socket`. */
  const SOCKETS = [
    ['Docker', '/var/run/docker.sock'],
    ['Podman as root', '/run/podman/podman.sock'],
    ['Podman without root', '/run/user/1000/podman/podman.sock'],
  ] as const
  // The form edits a copy; the drawer or row remounts it for another connection.
  const c = untrack(() => conn)
  let kind = $state<Kind>(c?.kind ?? 'ssh')
  let name = $state(c?.name ?? '')
  let target = $state(c?.kind === 'ssh' ? c.target : '')
  let port = $state(c?.kind === 'ssh' && c.port ? String(c.port) : '')
  let identity = $state((c?.kind === 'ssh' && c.identity) || '')
  let jump = $state((c?.kind === 'ssh' && c.jump) || '')
  let socket = $state(c?.kind === 'ssh' ? c.socket : '/var/run/docker.sock')
  let endpoint = $state((c?.kind === 'local' && c.endpoint) || '')
  let advanced = $state(c?.kind === 'ssh' && !!(c.port || c.identity || c.jump || c.socket !== '/var/run/docker.sock'))
  let busy = $state(false)
  let steps = $state<TestStep[] | 'running' | null>(null)
  let error = $state<string | null>(null)
  let failed = $state(false)

  function body() {
    if (kind === 'local') return { name: name.trim() || 'This computer', kind, endpoint: endpoint.trim() || null }
    // `user@host:2222` works too: ssh itself does not take a port in the target.
    let t = target.trim()
    let p = port.trim()
    const m = /^([^:[\]]+):(\d{1,5})$/.exec(t)
    if (m && !p) {
      t = m[1]
      p = m[2]
    }
    return {
      name: name.trim() || t,
      kind,
      target: t,
      port: p ? Number(p) : null,
      identity: identity.trim() || null,
      jump: jump.trim() || null,
      socket: socket.trim() || '/var/run/docker.sock',
    }
  }

  async function save(e: SubmitEvent | null, force = false) {
    e?.preventDefault()
    busy = true
    error = null
    try {
      if (!force) {
        steps = 'running'
        const r = await post<TestStep[]>('/connections/test', body())
        steps = r
        failed = r.some((s) => !s.ok)
        if (failed) return
      }
      if (conn) {
        await put<Connection>(`/connections/${encodeURIComponent(conn.id)}`, body())
        await app.reloadConnections()
        app.notify(`Saved ${body().name}.`)
      } else {
        const saved = await post<Connection>('/connections', body())
        await app.reloadConnections()
        app.select(saved.id)
        app.notify(`Added ${saved.name}. It is now the active connection.`)
      }
      ondone()
    } catch (err) {
      error = app.describe(err)
      if (steps === 'running') steps = null
    } finally {
      busy = false
    }
  }
</script>

<form class="form" onsubmit={(e) => save(e)}>
  {#if !conn}
    <div class="full">
      <div class="seg" role="group" aria-label="Type">
        <button type="button" aria-pressed={kind === 'ssh'} onclick={() => (kind = 'ssh')}>SSH to a server</button>
        <button type="button" aria-pressed={kind === 'local'} onclick={() => (kind = 'local')}>This computer</button>
      </div>
    </div>
  {/if}
  <label class="field"><span>Name</span><input bind:value={name} placeholder={kind === 'ssh' ? 'e.g. prod-server' : 'This computer'} autocomplete="off" /></label>
  {#if kind === 'ssh'}
    <label class="field">
      <span>Host</span>
      <input bind:value={target} placeholder="user@host, user@host:2222 or an alias" autocomplete="off" spellcheck="false" required />
      <small>Aliases from <span class="mono">~/.ssh/config</span> work. Stackline runs your own ssh; keys are never copied.</small>
    </label>
    <details class="adv full" bind:open={advanced}>
      <summary><Icon name="chev" size={12} />Advanced: port, key file, jump host, Docker or Podman socket</summary>
      <div class="form">
        <label class="field"><span>Port <span class="opt">optional</span></span><input bind:value={port} inputmode="numeric" placeholder="from ~/.ssh/config, else 22" autocomplete="off" /></label>
        <label class="field"><span>Key file <span class="opt">optional</span></span><input bind:value={identity} placeholder="~/.ssh/id_ed25519" autocomplete="off" spellcheck="false" /></label>
        <label class="field"><span>Jump host <span class="opt">optional</span></span><input bind:value={jump} placeholder="user@bastion:22" autocomplete="off" spellcheck="false" /></label>
        <label class="field">
          <span>Docker or Podman socket on the server</span>
          <input bind:value={socket} autocomplete="off" spellcheck="false" />
          <small class="presets">
            {#each SOCKETS as [label, path] (label)}<button type="button" class="linkish" aria-pressed={socket === path} onclick={() => (socket = path)}>{label}</button>{/each}
          </small>
          {#if socket.includes('/run/user/')}<small>Podman without root listens under the user's id: <span class="mono">id -u</span> on the server prints it.</small>{/if}
        </label>
      </div>
    </details>
  {:else}
    <label class="field">
      <span>Docker endpoint <span class="opt">optional</span></span>
      <input bind:value={endpoint} placeholder="unix:///var/run/docker.sock" autocomplete="off" spellcheck="false" />
      <small>Empty uses Docker's default socket, or <span class="mono">DOCKER_HOST</span> when it is set, or Podman's when there is no Docker.</small>
    </label>
  {/if}
  <div class="full" style="display:flex;gap:8px;flex-wrap:wrap;align-items:center">
    <button class="btn pri" type="submit" disabled={busy}>{busy ? 'Testing…' : conn ? 'Test and save changes' : 'Test and save'}</button>
    {#if failed && !busy}<button class="btn" type="button" onclick={() => save(null, true)}>Save anyway</button>{/if}
    <button class="btn ghost" type="button" onclick={ondone}>Cancel</button>
    {#if kind === 'ssh'}<span class="hint">The SSH user needs read access to the socket: for Docker usually through the <span class="mono">docker</span> group, for Podman without root it is the user's own.</span>{/if}
  </div>
  {#if error}<div class="full err-banner">{error}</div>{/if}
  {#if steps}<div class="full"><TestSteps {steps} /></div>{/if}
</form>

<style>
  .presets { display: flex; gap: 10px; flex-wrap: wrap }
  .presets button[aria-pressed='true'] { font-weight: 600; text-decoration: none }
</style>
