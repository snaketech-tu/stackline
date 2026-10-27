<script lang="ts">
  import { app } from '../lib/state.svelte'
  import { api, query } from '../lib/api'
  import { ago, bytes, pct } from '../lib/format'
  import { svcState, taskPill } from '../lib/health'
  import type { LogResponse } from '../lib/types'
  import Drawer from './Drawer.svelte'
  import Histogram from './Histogram.svelte'
  import PinButton from './PinButton.svelte'

  let { key }: { key: string } = $props()
  let s = $derived(app.services.find((x) => x.key === key))

  let hist = $state<LogResponse | null>(null)
  let end = $state(Date.now())
  $effect(() => {
    const name = s?.fullName
    if (!name) return
    const to = Date.now()
    api<LogResponse>(`/c/${encodeURIComponent(app.activeId)}/logs?${query({ from: to - 6 * 3600e3, to, service: name, levels: 'ERROR,WARN', buckets: 36, limit: 1 })}`)
      .then((r) => {
        hist = r
        end = to
      })
      .catch(() => {})
  })

  function openLogs() {
    if (!s) return
    app.logs.reset()
    app.logs.service = s.fullName
    app.drawer = null
    app.go('logs')
  }
</script>

<Drawer title={s ? s.fullName : key}>
  {#snippet actions()}<PinButton {key} label />{/snippet}
  {#if !s}
    <div class="empty">This service is no longer running on {app.active?.name}.</div>
  {:else}
    {@const [cls] = svcState(s)}
    <div style="display:flex;gap:8px;flex-wrap:wrap;align-items:center">
      <span class="pill {cls}">{s.running}/{s.desired}</span><span class="pill">{s.mode}</span><span class="kind">{s.kind}</span>
      {#if s.updatedAt}<span class="muted">updated {ago(s.updatedAt, app.now)}</span>{/if}
      <span class="spacer"></span>
      <button class="btn pri" onclick={openLogs}>Open logs</button>
    </div>
    <dl class="kv">
      <dt>Image</dt><dd>{s.image}</dd>
      <dt>Ports</dt><dd>{s.ports.join(', ') || '–'}</dd>
      <dt>Stack</dt><dd>{s.stack}</dd>
      <dt>Usage</dt><dd>CPU {pct(s.cpuPercent, 1)} · memory {bytes(s.memUsed)}</dd>
      {#if s.kind === 'swarm'}<dt>Service ID</dt><dd>{s.id.slice(0, 12)}</dd>{/if}
    </dl>
    <div>
      <h4>{s.kind === 'swarm' ? 'Tasks' : 'Containers'}</h4>
      <div class="tw">
        <table>
          <thead><tr><th>Replica</th><th>Node</th><th>State</th><th>Since</th><th class="hide-sm">CPU / Mem</th></tr></thead>
          <tbody>
            {#each s.tasks as t (t.id)}
              {@const [pc, label] = taskPill(t)}
              <tr>
                <td class="mono">{t.slot}</td>
                <td class="mono">{t.node}</td>
                <td>
                  <span class="pill {pc}">{label}</span>{#if t.exitCode}<span class="muted mono" style="font-size:11.5px"> exit {t.exitCode}</span>{/if}
                  {#if t.error || (t.state !== 'running' && t.message)}<div class="muted" style="font-size:12px;margin-top:3px;white-space:pre-wrap">{t.error || t.message}</div>{/if}
                </td>
                <td class="muted">{t.since ? ago(t.since, app.now) : '–'}</td>
                <td class="hide-sm mono">{pct(t.cpuPercent, 1)} · {bytes(t.memUsed)}</td>
              </tr>
            {:else}
              <tr><td colspan="5" class="muted">No tasks.</td></tr>
            {/each}
          </tbody>
        </table>
      </div>
    </div>
    <div>
      <h4>Errors &amp; warnings · last 6h</h4>
      {#if hist}<Histogram buckets={hist.result.histogram} from={end - 6 * 3600e3} to={end} levels={['WARN', 'ERROR']} height={110} />{/if}
    </div>
  {/if}
</Drawer>
