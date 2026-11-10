<script lang="ts">
  // `?`: every key Stackline knows, in one place.
  import { app } from '../lib/state.svelte'

  const mac = /Mac|iPhone|iPad/.test(navigator.platform)
  const GROUPS: [string, [string, string][]][] = [
    ['Anywhere', [
      [`${mac ? '⌘' : 'Ctrl'} K  :`, 'Search or run a command'],
      ['?', 'This list'],
      ['g o', 'Overview'], ['g k', 'Stacks'], ['g s', 'Services'], ['g t', 'Tasks'], ['g n', 'Nodes'], ['g l', 'Logs'],
      ['g c', 'Connections'], ['g p', 'Preferences'],
      ['Alt ←  Alt →', 'Back and forward'],
      ['w', 'Show or shrink the log workspace'],
      ['Esc', 'Close the inspector, a menu or a dialog'],
    ]],
    ['In a list', [
      ['j  k  ↑  ↓', 'Move the cursor'],
      ['Home  End', 'First and last row'],
      ['Enter', 'Open: a stack’s services, a service’s tasks, a node’s tasks'],
      ['d', 'Inspect the row beside the list (or click it)'],
      ['l', 'Follow its logs in the workspace below'],
      ['e', 'Errors of the service'],
      ['o', 'Open its lines on the Logs page'],
      ['p', 'Pin or unpin the service'],
      ['t', 'Tasks of the stack'],
      ['n', 'Tasks on the node of a task'],
      ['s', 'Narrow every list to the stack'],
      ['/', 'Filter the list'],
    ]],
    ['When changes are allowed', [
      ['r', 'Restart the service or container'],
      ['x', 'Stop it (Compose and lone containers)'],
      ['u', 'Start it again'],
      ['c', 'Scale a swarm service'],
      ['Enter', 'Confirm in the dialog, which shows the docker command'],
    ]],
    ['In the inspector', [
      ['[  ]', 'Previous and next service'],
    ]],
    ['On the Logs page', [
      ['/', 'Search'],
      ['j  k', 'Move through the lines'],
      ['Enter', 'Expand a line'],
      ['c', 'Copy a line'],
      ['f', 'Fill the window with the stream'],
      ['t', 'Type or pick a time range'],
      ['[  ]', 'The range before or after this one'],
      ['-', 'Zoom out to twice the range'],
    ]],
  ]
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="pal-wrap" onclick={(e) => { if (e.target === e.currentTarget) app.help = false }}>
  <div class="pal help-ov" role="dialog" aria-modal="true" aria-label="Keys">
    <div class="in"><b>Keys</b><span class="spacer"></span><span class="muted" style="font-size:12px">{app.access.writable ? 'Changes are allowed here. Each one asks first.' : 'Read-only: no key changes your containers.'}</span><kbd>Esc</kbd></div>
    <div class="help-grid">
      {#each GROUPS as [title, keys] (title)}
        <section>
          <h4>{title}</h4>
          <dl>
            {#each keys as [k, what] (k)}<dt>{#each k.split('  ') as part, i (i)}{#if i}<span class="or">/</span>{/if}{#each part.split(' ') as x, j (j)}<kbd>{x}</kbd>{/each}{/each}</dt><dd>{what}</dd>{/each}
          </dl>
        </section>
      {/each}
    </div>
  </div>
</div>
