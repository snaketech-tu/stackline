// Thin client for the local Stackline API. The server signs the browser in with
// a cookie; during development a `?token=` in the page URL is sent as a header.

import type { Deploy, DockerEvent, LogResponse } from './types'

let devToken: string | null = null
try {
  const u = new URL(location.href)
  const t = u.searchParams.get('token')
  if (t) {
    sessionStorage.setItem('stackline-token', t)
    u.searchParams.delete('token')
    history.replaceState(null, '', u.toString())
  }
  devToken = sessionStorage.getItem('stackline-token')
} catch {
  devToken = null
}

export class ApiError extends Error {
  constructor(public status: number, message: string) {
    super(message)
  }
}

/** A raw API response, for downloads. Throws on errors like `api`. */
export async function apiFetch(path: string, init: RequestInit = {}): Promise<Response> {
  const headers = new Headers(init.headers)
  if (devToken) headers.set('x-stackline-token', devToken)
  if (init.body && !headers.has('content-type')) headers.set('content-type', 'application/json')
  const res = await fetch('/api' + path, { ...init, headers, credentials: 'same-origin' })
  if (!res.ok) {
    let msg = res.statusText
    try {
      const body = await res.json()
      msg = body.error ?? msg
    } catch {
      msg = (await res.text().catch(() => '')) || msg
    }
    throw new ApiError(res.status, msg)
  }
  return res
}

export async function api<T>(path: string, init: RequestInit = {}): Promise<T> {
  const res = await apiFetch(path, init)
  if (res.status === 204) return undefined as T
  return res.json() as Promise<T>
}

export const post = <T>(path: string, body: unknown) => api<T>(path, { method: 'POST', body: JSON.stringify(body) })
export const put = <T>(path: string, body: unknown) => api<T>(path, { method: 'PUT', body: JSON.stringify(body) })
export const del = (path: string) => api<void>(path, { method: 'DELETE' })

export function query(params: Record<string, string | number | undefined | null>): string {
  const q = new URLSearchParams()
  for (const [k, v] of Object.entries(params)) if (v !== undefined && v !== null && v !== '') q.set(k, String(v))
  return q.toString()
}

/** Parameters of a log query. Times are Unix milliseconds; `levels`, `stack` and `service` are comma lists. */
export interface LogParams {
  from: number; to: number; levels?: string; stack?: string; service?: string; q?: string; buckets?: number; limit?: number
}

/** Queries the stored logs of a connection. */
export function fetchLogs(conn: string, p: LogParams): Promise<LogResponse> {
  return api<LogResponse>(`/c/${encodeURIComponent(conn)}/logs?${query({ ...p, from: Math.floor(p.from), to: Math.ceil(p.to) })}`)
}

/** Container starts, stops and crashes, filtered by the sources of `p`. */
export async function fetchEvents(conn: string, p: LogParams): Promise<DockerEvent[]> {
  const { from, to, stack, service, q, limit } = p
  const r = await api<{ events: DockerEvent[] }>(
    `/c/${encodeURIComponent(conn)}/events?${query({ from: Math.floor(from), to: Math.ceil(to), stack, service, q, limit })}`,
  )
  return r.events
}

/** Deploys of the range, newest first, with the errors that are new since each one. */
export async function fetchDeploys(conn: string, p: LogParams): Promise<Deploy[]> {
  const { from, to, stack, service } = p
  const r = await api<{ deploys: Deploy[] }>(`/c/${encodeURIComponent(conn)}/deploys?${query({ from: Math.floor(from), to: Math.ceil(to), stack, service })}`)
  return r.deploys
}

/** Lines already shown keep their objects, so their rows are not drawn again. */
export function keepEntries(prev: LogResponse | null, next: LogResponse): LogResponse {
  if (!prev) return next
  const known = new Map(prev.result.entries.map((e) => [e.id, e]))
  next.result.entries = next.result.entries.map((e) => known.get(e.id) ?? e)
  return next
}

export function hasDevToken(): boolean {
  return !!devToken
}
