import type { DockerEvent, EventKind } from './types'

/** Exits nobody asked for: these are what people look for when debugging. */
export const CRASH_KINDS: ReadonlySet<EventKind> = new Set(['crash', 'oom', 'failed'])

export const isCrash = (e: DockerEvent) => CRASH_KINDS.has(e.kind)

/** What happened, in a few words: "crashed · exit 1", "out of memory". */
export function eventLabel(e: DockerEvent): string {
  const code = e.exitCode !== null ? ` · exit ${e.exitCode}` : ''
  switch (e.kind) {
    case 'start':
      return 'started'
    case 'exit':
      return 'exited' + code
    case 'stop':
      return 'stopped' + code
    case 'crash':
      return 'crashed' + code
    case 'oom':
      return 'out of memory'
    case 'unhealthy':
      return 'health check failed'
    case 'failed':
      return (e.detail ? `failed: ${e.detail}` : 'failed') + code
    case 'deploy':
      return e.detail ? `deployed: ${e.detail}` : 'deployed'
  }
}

/** The colour of an event's mark on a time graph. */
export function eventColor(e: DockerEvent): string {
  if (isCrash(e)) return 'var(--err)'
  if (e.kind === 'unhealthy') return 'var(--warn)'
  if (e.kind === 'deploy') return 'var(--accent)'
  return 'var(--muted)'
}

/** Who an event happened to: `shop_api #2`, or just the service for a deploy. */
export const eventWho = (e: DockerEvent) => (e.kind === 'deploy' ? e.service : `${e.service} #${e.replica}`)

/** The 2 minutes before an event and 15 s after it, in milliseconds. */
export function beforeEvent(e: DockerEvent): [number, number] {
  return [e.ts / 1000 - 120_000, e.ts / 1000 + 15_000]
}
