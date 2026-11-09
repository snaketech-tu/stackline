// What each list (stacks, services, tasks, nodes) remembers while someone
// moves around the app: the filter text, the tab, the sort and the row the
// cursor is on. Back to a list shows it the way it was left.

import { load } from './storage'

export type ListTab = 'all' | 'attention' | 'pinned' | 'running'
export type ListView = 'stacks' | 'services' | 'tasks' | 'nodes'

export class ListState {
  q = $state('')
  tab = $state<ListTab>('all')
  /** Column id and direction; null is the list's own order. */
  sort = $state<{ col: string; desc: boolean } | null>(null)
  /** Key of the row the cursor is on. Shown only once someone moved it (a key, a click, a link). */
  cursor = $state<string | null>(null)

  /** Sorts by a column: first ascending, then descending, then back to the list's own order. */
  sortBy(col: string) {
    if (this.sort?.col !== col) this.sort = { col, desc: false }
    else if (!this.sort.desc) this.sort = { col, desc: true }
    else this.sort = null
  }

  reset() {
    this.q = ''
    this.tab = 'all'
    this.sort = null
    this.cursor = null
  }
}

const VIEWS: ListView[] = ['stacks', 'services', 'tasks', 'nodes']

/** The lists, with the filter and tab each had when Stackline was last open. */
export function newLists(): Record<ListView, ListState> {
  const out = {} as Record<ListView, ListState>
  for (const v of VIEWS) {
    const l = (out[v] = new ListState())
    try {
      const saved = JSON.parse(load(`stackline-list-${v}`) ?? 'null')
      if (saved && typeof saved.q === 'string') l.q = saved.q
      if (saved && ['all', 'attention', 'pinned', 'running'].includes(saved.tab)) l.tab = saved.tab
    } catch {
      // A damaged entry means the defaults.
    }
  }
  return out
}

/** Compares two sort values: numbers as numbers, the rest as text; empty values last. */
export function compare(a: string | number | null | undefined, b: string | number | null | undefined): number {
  const ea = a === null || a === undefined || a === ''
  const eb = b === null || b === undefined || b === ''
  if (ea || eb) return ea === eb ? 0 : ea ? 1 : -1
  if (typeof a === 'number' && typeof b === 'number') return a - b
  return String(a).localeCompare(String(b), undefined, { numeric: true })
}
