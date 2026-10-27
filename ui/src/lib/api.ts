// Thin client for the local swarmdash API. The server signs the browser in with
// a cookie; during development a `?token=` in the page URL is sent as a header.

let devToken: string | null = null
try {
  const u = new URL(location.href)
  const t = u.searchParams.get('token')
  if (t) {
    sessionStorage.setItem('swarmdash-token', t)
    u.searchParams.delete('token')
    history.replaceState(null, '', u.toString())
  }
  devToken = sessionStorage.getItem('swarmdash-token')
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
  if (devToken) headers.set('x-swarmdash-token', devToken)
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

export function hasDevToken(): boolean {
  return !!devToken
}
