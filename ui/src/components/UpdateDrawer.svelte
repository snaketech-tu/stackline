<script lang="ts">
  import { app } from '../lib/state.svelte'
  import { bytes } from '../lib/format'
  import { markdown } from '../lib/markdown'
  import Drawer from './Drawer.svelte'
  import Icon from './Icon.svelte'

  let u = $derived(app.update)
  let latest = $derived(u?.releases[0])
  let busy = $derived(!!u && ['downloading', 'installing', 'restarting'].includes(u.phase))
  let starting = $state(false)
  let pct = $derived(u?.total ? Math.min(100, (u.received / u.total) * 100) : null)

  async function install() {
    starting = true
    try {
      await app.installUpdate()
    } catch (e) {
      app.notify(app.describe(e), 'err')
    } finally {
      starting = false
    }
  }

  async function check() {
    try {
      await app.checkUpdate()
    } catch (e) {
      app.notify(app.describe(e), 'err')
    }
  }

  function later() {
    app.dismissUpdate()
    app.drawer = null
  }

  // Links in release notes open in the system browser, also from the desktop app.
  function openLink(e: MouseEvent) {
    const a = (e.target as HTMLElement).closest('a')
    if (!a) return
    e.preventDefault()
    app.open(a.href)
  }

  const date = (s: string | null) => (s ? new Date(s).toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' }) : '')
</script>

<Drawer title="Update swarmdash">
  {#if !u}
    <div class="center-empty"><span class="spinner"></span></div>
  {:else if !latest}
    <div class="update-card">
      <span class="big">You're up to date</span>
      <span class="muted">swarmdash {u.current} is the newest version.</span>
      <div><button class="btn" onclick={check} disabled={u.phase === 'checking'}>{u.phase === 'checking' ? 'Checking…' : 'Check again'}</button></div>
    </div>
    {#if u.error}<div class="err-banner">{u.error}</div>{/if}
  {:else}
    <div class="update-card">
      <span class="big">swarmdash {latest.version} is available</span>
      <span class="muted">You have {u.current}.{#if u.releases.length > 1}{' '}That is {u.releases.length} releases behind.{/if}</span>

      {#if u.phase === 'downloading'}
        <div class="progress"><i style:width="{pct ?? 0}%"></i></div>
        <span class="muted mono" style="font-size:12px">Downloading {bytes(u.received)}{#if u.total}{' '}of {bytes(u.total)}{/if}…</span>
      {:else if u.phase === 'installing'}
        <div class="progress indet"><i></i></div>
        <span class="muted" style="font-size:12.5px">Checking the signature and installing…</span>
      {:else if u.phase === 'restarting'}
        <div class="progress indet"><i></i></div>
        <span class="muted" style="font-size:12.5px">Restarting swarmdash. This page reloads by itself in a moment.</span>
      {:else if u.canInstall}
        <div style="display:flex;gap:8px;flex-wrap:wrap">
          <button class="btn pri" onclick={install} disabled={starting}><Icon name="download" size={14} />Update and restart</button>
          <button class="btn ghost" onclick={later}>Later</button>
        </div>
        <span class="muted" style="font-size:12px">Downloads the new version, checks its signature, installs it and restarts swarmdash. Your connections, pins and stored logs are kept.</span>
      {:else}
        <span style="font-size:13px">{u.installHint}</span>
        <div style="display:flex;gap:8px;flex-wrap:wrap">
          <button class="btn pri" onclick={() => app.open(latest!.url)}><Icon name="external" size={14} />Open the download page</button>
          <button class="btn ghost" onclick={later}>Later</button>
        </div>
      {/if}
    </div>
    {#if u.error && !busy}<div class="err-banner">{u.error}</div>{/if}

    <div>
      <h4>What's new</h4>
      <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
      <div class="notes" onclick={openLink}>
        {#each u.releases as r (r.version)}
          <div class="rel">
            <h4>{r.version}{#if r.name && r.name !== `v${r.version}` && r.name !== r.version}<span class="muted" style="font-weight:500;font-size:13px">{r.name}</span>{/if}<span class="spacer"></span><span class="muted" style="font-weight:400;font-size:12px">{date(r.publishedAt)}</span></h4>
            <div class="md">
              {#if r.notes.trim()}{@html markdown(r.notes)}{:else}<p class="muted">No notes for this release.</p>{/if}
            </div>
          </div>
        {/each}
      </div>
      <p style="margin:14px 0 0"><button class="linkish" onclick={() => app.open(u!.releasesUrl)}>All releases on GitHub</button></p>
    </div>
  {/if}
</Drawer>
