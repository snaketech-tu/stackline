<script lang="ts">
  // The connection switcher in the top bar. Its dot shows the connection only, not service health.
  import { app } from '../lib/state.svelte'
  import Icon from './Icon.svelte'
  import Popover from './Popover.svelte'

  let btn: HTMLButtonElement | undefined = $state()
  let open = $state(false)

  let dot = $derived(!app.conn || app.conn.status === 'connecting' ? 'off' : app.conn.status === 'error' ? 'err' : '')
  const target = (c: (typeof app.connections)[number]) => (c.kind === 'ssh' ? `${c.target}${c.port ? ':' + c.port : ''}` : c.endpoint || 'Default Docker socket')

  function pick(id: string) {
    open = false
    app.select(id)
  }
</script>

<button class="ctl connbtn" bind:this={btn} aria-haspopup="menu" aria-expanded={open} title="Switch connection" onclick={() => (open = !open)}>
  <span class="dot {dot}"></span><b>{app.active?.name ?? '…'}</b><Icon name="chevD" size={14} />
</button>
{#if open && btn}
  <Popover anchor={btn} label="Connections" onclose={() => (open = false)}>
    <div class="hd">Connections</div>
    {#each app.connections as c (c.id)}
      {@const active = c.id === app.activeId}
      <button class="it" class:sel={active} role="menuitemradio" aria-checked={active} onclick={() => pick(c.id)}>
        <span class="dot {active ? dot : 'off'}"></span>
        <span class="l"><b>{c.name}</b><br /><span class="muted mono" style="font-size:11px">{target(c)}</span></span>
        <span class="r">{active && app.conn?.latencyMs != null ? `${app.conn.latencyMs} ms` : c.kind === 'ssh' ? 'SSH' : 'Local'}</span>
      </button>
    {/each}
    <div class="sepl"></div>
    <button class="it" role="menuitem" onclick={() => { open = false; app.go('connections'); app.drawer = { kind: 'connection', id: null } }}><Icon name="plus" size={14} />Add a connection</button>
    <button class="it" role="menuitem" onclick={() => { open = false; app.go('connections') }}><Icon name="connections" size={14} />Manage connections</button>
  </Popover>
{/if}
