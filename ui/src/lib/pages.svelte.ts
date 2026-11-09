// The lines of a log stream, page by page: the newest page is fetched again
// on every refresh, older pages are added below it by "Show more".
//
// Older pages start right after the last line already shown (by time and line
// id), so they never skip or repeat a line, however many lines arrive while
// someone reads. Once older pages are shown, a refresh asks for every line
// newer than them, so the stream stays one unbroken run of lines.

import { fetchLogs, keepEntries, type LogParams } from './api'
import type { Entry, LogResponse } from './types'

/** How many lines a stream shows at first, how many "Show more" adds, and the most it shows at all. */
export const LINES = { first: 300, step: 500, max: 20000 }
/** The most lines a refresh above the older pages returns; more means they are too far behind. */
const ABOVE_MAX = 5000

const key = (e: Entry) => `${e.ts}:${e.id}`

export class LogPages {
  /** The newest lines, with the counts, graph and groups of the whole range. */
  head = $state.raw<LogResponse | null>(null)
  /** Older lines added by "Show more", newest first. */
  older = $state.raw<Entry[]>([])
  private olderMore = $state(false)
  loadingOlder = $state(false)
  /** Why the last "Show more" failed. */
  olderError = $state<string | null>(null)
  /** Bumped by `reset`, so answers for earlier filters are dropped. */
  private gen = 0

  /** Every line shown, newest first. */
  entries = $derived<Entry[]>(this.older.length ? [...(this.head?.result.entries ?? []), ...this.older] : (this.head?.result.entries ?? []))
  /** Older lines match the filters beyond the ones shown. */
  more = $derived(this.older.length ? this.olderMore : !!this.head?.result.more)
  /** The stream reached the most lines it shows; older ones need a narrower range. */
  full = $derived(this.entries.length >= LINES.max)

  /** Back to the first page, for new filters. */
  reset() {
    this.gen++
    this.older = []
    this.olderMore = false
    this.loadingOlder = false
    this.olderError = null
  }

  /** Forgets everything, the newest page too. */
  clear() {
    this.reset()
    this.head = null
  }

  /** The parameters for a refresh of the newest lines. */
  headParams(p: LogParams, first = LINES.first): LogParams {
    return this.older.length ? { ...p, after: key(this.older[0]), limit: ABOVE_MAX } : { ...p, limit: first }
  }

  /** Takes the answer of a refresh made with `headParams`. */
  takeHead(r: LogResponse) {
    // So many new lines that they no longer meet the older pages: start over at the newest.
    if (this.older.length && r.result.more) {
      this.reset()
      r.result.entries = r.result.entries.slice(0, LINES.first)
    }
    this.head = keepEntries(this.head, r)
  }

  /** Adds the next older page below the lines shown. True when it added lines. */
  async loadOlder(conn: string, p: LogParams, describe: (e: unknown) => string): Promise<boolean> {
    const last = this.entries[this.entries.length - 1]
    if (!last || this.loadingOlder || !this.more || this.full || this.olderError) return false
    const gen = this.gen
    this.loadingOlder = true
    this.olderError = null
    try {
      const step = Math.min(LINES.step, LINES.max - this.entries.length)
      const r = await fetchLogs(conn, { ...p, before: key(last), limit: step, buckets: 1 })
      if (gen !== this.gen) return false
      this.older = [...this.older, ...r.result.entries]
      this.olderMore = r.result.more
      return r.result.entries.length > 0
    } catch (e) {
      if (gen === this.gen) this.olderError = describe(e)
      return false
    } finally {
      if (gen === this.gen) this.loadingOlder = false
    }
  }

  /** Lets "Try again" ask once more after an error. */
  retry(conn: string, p: LogParams, describe: (e: unknown) => string) {
    this.olderError = null
    return this.loadOlder(conn, p, describe)
  }
}

/**
 * Calls `load` while the element is in view, or close to it: put at the end
 * of a stream, older lines come in as someone scrolls down to them. It goes
 * on as long as `load` adds lines and the end is still in view.
 */
export function nearEnd(el: HTMLElement, load: () => Promise<boolean> | boolean) {
  let visible = false
  let running = false
  async function run() {
    if (running) return
    running = true
    try {
      // Wait for the added rows to be laid out before looking again.
      while (visible && (await load())) await new Promise((r) => requestAnimationFrame(() => setTimeout(r, 50)))
    } finally {
      running = false
    }
  }
  const io = new IntersectionObserver(
    (list) => {
      visible = list.some((x) => x.isIntersecting)
      if (visible) run()
    },
    { rootMargin: '0px 0px 600px 0px' },
  )
  io.observe(el)
  return {
    update(next: typeof load) {
      load = next
    },
    destroy() {
      io.disconnect()
    },
  }
}
