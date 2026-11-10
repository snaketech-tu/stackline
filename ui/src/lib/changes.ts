// The changes a list or the inspector offers once a connection allows them:
// restart, stop, start and scale. Each one asks first (see `app.act`).

import type { Act } from '../components/ResourceList.svelte'
import { app } from './state.svelte'
import type { ServiceRef, Task } from './types'

const swarm = (s: ServiceRef) => s.kind === 'swarm'
// The same container states the server acts on (see `actions.rs`).
const stoppable = (t: Task) => !!t.containerId && ['running', 'restarting', 'paused'].includes(t.state)
const startable = (t: Task) => !!t.containerId && ['exited', 'created'].includes(t.state)
const restartable = (t: Task) => !!t.containerId && !['dead', 'removing'].includes(t.state)
const req = (s: ServiceRef) => ({ service: s.fullName, stack: s.stack })

export function restart(s: ServiceRef) {
  app.act({ action: 'restart', ...req(s) }, swarm(s) ? `Restart the tasks of ${s.fullName}?` : `Restart ${s.fullName}?`)
}
export function stop(s: ServiceRef) {
  app.act({ action: 'stop', ...req(s) }, `Stop ${s.fullName}?`)
}
export function start(s: ServiceRef) {
  app.act({ action: 'start', ...req(s) }, `Start ${s.fullName}?`)
}
export function scale(s: ServiceRef) {
  app.act({ action: 'scale', ...req(s), replicas: s.desired }, `Scale ${s.fullName}`)
}

/** Which of the changes fit a service. */
export const can = {
  restart: (s: ServiceRef) => swarm(s) || s.tasks.some(restartable),
  stop: (s: ServiceRef) => !swarm(s) && s.tasks.some(stoppable),
  start: (s: ServiceRef) => !swarm(s) && s.tasks.some(startable),
  scale: (s: ServiceRef) => swarm(s) && s.mode === 'replicated',
}

/** Leaves out the changes that fit none of the rows, such as Scale on a host without swarm. */
const fitting = <T>(rows: T[], acts: Act<T>[]) => acts.filter((a) => rows.some((r) => !a.when || a.when(r)))

/** The changes for rows of a list, offered only while the connection allows them. */
export function serviceActs<T>(rows: T[], of: (r: T) => ServiceRef): Act<T>[] {
  if (!app.access.writable) return []
  return fitting(rows, [
    { key: 'r', label: 'Restart', icon: 'restart', run: (r) => restart(of(r)), when: (r) => can.restart(of(r)) },
    { key: 'x', label: 'Stop', icon: 'stop', run: (r) => stop(of(r)), when: (r) => can.stop(of(r)) },
    { key: 'u', label: 'Start', icon: 'play', run: (r) => start(of(r)), when: (r) => can.start(of(r)) },
    { key: 'c', label: 'Scale', icon: 'scale', run: (r) => scale(of(r)), when: (r) => can.scale(of(r)) },
  ])
}

/** The changes for one container of a Compose service or a lone container. Swarm tasks go through their service. */
export function taskActs<T>(rows: T[], of: (r: T) => { s: ServiceRef; t: Task }): Act<T>[] {
  if (!app.access.writable) return []
  const one = (r: T, action: 'restart' | 'stop' | 'start', verb: string) => {
    const { s, t } = of(r)
    app.act({ action, ...req(s), replica: t.slot }, `${verb} ${t.containerName ?? `${s.fullName} #${t.slot}`}?`)
  }
  const container = (r: T, fits: (t: Task) => boolean) => !swarm(of(r).s) && fits(of(r).t)
  return fitting(rows, [
    { key: 'r', label: 'Restart', icon: 'restart', run: (r) => (swarm(of(r).s) ? restart(of(r).s) : one(r, 'restart', 'Restart')), when: (r) => swarm(of(r).s) || container(r, restartable) },
    { key: 'x', label: 'Stop', icon: 'stop', run: (r) => one(r, 'stop', 'Stop'), when: (r) => container(r, stoppable) },
    { key: 'u', label: 'Start', icon: 'play', run: (r) => one(r, 'start', 'Start'), when: (r) => container(r, startable) },
  ])
}
