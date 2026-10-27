<script lang="ts">
  import type { TestStep } from '../lib/types'

  let { steps }: { steps: TestStep[] | 'running' } = $props()
</script>

<div class="steps">
  {#if steps === 'running'}
    <div><span class="spinner" style="width:14px;height:14px;border-width:2px"></span>Testing…</div>
  {:else}
    {#each steps as s, i (i)}
      <div style="align-items:flex-start">
        <span class="dot {s.ok ? '' : 'err'}" style="margin-top:6px"></span>
        <span>{s.label}{#if s.ms} <span class="muted mono" style="font-size:11px">{s.ms} ms</span>{/if}
          {#if s.detail}<br /><span class="muted" style="white-space:pre-wrap;font-size:12.5px">{s.detail}</span>{/if}</span>
      </div>
    {/each}
  {/if}
</div>
