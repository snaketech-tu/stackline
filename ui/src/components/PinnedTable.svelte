<script lang="ts">
  import { app } from '../lib/state.svelte'
  import { ago, bytes, pct } from '../lib/format'
  import { svcState } from '../lib/health'
  import type { ServiceRef } from '../lib/types'
  import PinButton from './PinButton.svelte'

  let { services }: { services: ServiceRef[] } = $props()
</script>

<div class="tw">
  <table>
    <thead>
      <tr><th class="pincell"><span class="sr">Pin</span></th><th>Service</th><th>Stack</th><th>Replicas</th><th class="hide-sm">CPU</th><th class="hide-sm">Memory</th><th>Errors 1h</th><th class="hide-sm">Updated</th></tr>
    </thead>
    <tbody>
      {#each services as s (s.key)}
        {@const [cls] = svcState(s)}
        {@const e = app.errors?.byService[s.fullName] ?? 0}
        <tr class="click" onclick={() => (app.drawer = { kind: 'service', key: s.key })}>
          <td class="pincell"><PinButton key={s.key} /></td>
          <td><b>{s.name}</b></td>
          <td class="muted">{s.stack}</td>
          <td><span class="pill {cls}">{s.running}/{s.desired}</span></td>
          <td class="hide-sm mono">{pct(s.cpuPercent, 1)}</td>
          <td class="hide-sm mono">{bytes(s.memUsed)}</td>
          <td class="mono" style:color={e ? 'var(--err)' : 'var(--muted)'}>{e}</td>
          <td class="hide-sm muted">{s.updatedAt ? ago(s.updatedAt, app.now) : '–'}</td>
        </tr>
      {/each}
    </tbody>
  </table>
</div>
