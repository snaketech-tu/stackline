<script lang="ts">
  import { app } from '../lib/state.svelte'
  import ConnectionForm from './ConnectionForm.svelte'
  import Drawer from './Drawer.svelte'

  let { id }: { id: string | null } = $props()
  let conn = $derived(id ? (app.connections.find((c) => c.id === id) ?? null) : null)
</script>

<Drawer title={conn ? `Edit ${conn.name}` : 'Add a connection'}>
  <ConnectionForm {conn} ondone={() => (app.drawer = null)} />
</Drawer>
