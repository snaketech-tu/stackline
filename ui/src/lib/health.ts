import type { Node, Service, Task } from './types'

/** Pill class and label for a service's replica health. */
export function svcState(s: Pick<Service, 'running' | 'desired'>): [string, string] {
  if (s.desired === 0) return ['', s.running ? 'running' : 'stopped']
  if (s.running === 0) return ['err', 'failing']
  if (s.running < s.desired) return ['warn', 'degraded']
  return ['ok', 'running']
}

export function taskPill(t: Task): [string, string] {
  if (t.state === 'running') return [t.health === 'unhealthy' ? 'warn' : 'ok', t.health === 'unhealthy' ? 'unhealthy' : 'running']
  if (t.state === 'failed' || t.state === 'rejected' || (t.state === 'exited' && t.exitCode)) return ['err', t.state]
  if (t.state === 'complete' || t.state === 'shutdown' || t.state === 'exited') return ['', t.state]
  return ['warn', t.state]
}

export function nodeRole(n: Node): string {
  if (n.role === 'manager') return n.leader ? 'Manager · leader' : 'Manager'
  if (n.role === 'worker') return 'Worker'
  if (n.role === 'standalone') return 'Docker host'
  return n.role
}

export function nodeDot(n: Node): string {
  if (n.state !== 'ready') return 'err'
  return n.availability === 'active' ? '' : 'warn'
}

export const memPct = (used: number | null, total: number) => (used === null || !total ? null : (used / total) * 100)
