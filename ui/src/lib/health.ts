import type { Node, Service, Task } from './types'

type Svc = Pick<Service, 'running' | 'desired' | 'tasks'>

/** A replica runs but its health check fails. */
const unhealthy = (s: Svc) => s.tasks.some((t) => t.state === 'running' && t.health === 'unhealthy')

/** Fewer replicas run than should, or one of them is unhealthy. */
export const svcProblem = (s: Svc) => s.running < s.desired || unhealthy(s)

/** Stopped on purpose: scaled to 0, stopped, or a job that finished. */
const stopped = (s: Svc) => s.desired === 0 && s.running === 0

/** Pill class and label for a service's replica health. */
export function svcState(s: Svc): [string, string] {
  if (s.desired === 0) return ['', s.running ? 'running' : 'stopped']
  if (s.running === 0) return ['err', 'failing']
  if (s.running < s.desired) return ['warn', 'degraded']
  if (unhealthy(s)) return ['warn', 'unhealthy']
  return ['ok', 'running']
}

/** Pill class and label for a stack, and how many of its services are stopped on purpose. */
export function stackState(services: Svc[]): { cls: string; label: string; stopped: number } {
  const n = services.filter(stopped).length
  if (services.some((s) => s.desired > 0 && s.running === 0)) return { cls: 'err', label: 'failing', stopped: n }
  if (services.some(svcProblem)) return { cls: 'warn', label: 'degraded', stopped: n }
  if (n && n === services.length) return { cls: '', label: 'stopped', stopped: 0 }
  return { cls: 'ok', label: 'healthy', stopped: n }
}

export function taskPill(t: Task): [string, string] {
  if (t.state === 'running') return [t.health === 'unhealthy' ? 'warn' : 'ok', t.health === 'unhealthy' ? 'unhealthy' : 'running']
  // A stopped container keeps its exit code, but was stopped on purpose.
  if (t.state === 'failed' || t.state === 'rejected' || (t.state === 'exited' && t.exitCode && t.desiredState === 'running')) return ['err', t.state]
  // A slot that keeps failing sits in ready or starting between attempts.
  if (t.failures > 0 && !['complete', 'shutdown', 'exited'].includes(t.state)) return ['err', 'restarting']
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
