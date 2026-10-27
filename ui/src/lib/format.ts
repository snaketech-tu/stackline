const pad = (n: number, l = 2) => String(n).padStart(l, '0')

export function hhmm(t: number): string {
  const d = new Date(t)
  return pad(d.getHours()) + ':' + pad(d.getMinutes())
}

export function dayHhmm(t: number, span: number): string {
  const d = new Date(t)
  if (span <= 36 * 3600e3) return hhmm(t)
  return `${d.toLocaleDateString(undefined, { weekday: 'short' })} ${hhmm(t)}`
}

export function hms(t: number): string {
  const d = new Date(t)
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}.${pad(d.getMilliseconds(), 3)}`
}

export function ago(t: number | null | undefined, now = Date.now()): string {
  if (!t) return '–'
  const s = Math.max(1, Math.round((now - t) / 1000))
  if (s < 60) return s + 's ago'
  const m = Math.round(s / 60)
  if (m < 60) return m + 'm ago'
  const h = Math.round(m / 60)
  if (h < 48) return h + 'h ago'
  return Math.round(h / 24) + 'd ago'
}

export function bytes(n: number | null | undefined): string {
  if (n === null || n === undefined) return '–'
  const u = ['B', 'KB', 'MB', 'GB', 'TB']
  let i = 0
  let v = n
  while (v >= 1024 && i < u.length - 1) {
    v /= 1024
    i++
  }
  return `${v >= 100 || i === 0 ? v.toFixed(0) : v.toFixed(1)} ${u[i]}`
}

export function pct(n: number | null | undefined, digits = 0): string {
  return n === null || n === undefined ? '–' : n.toFixed(digits) + '%'
}

export function plural(n: number, word: string, many = word + 's'): string {
  return `${n} ${n === 1 ? word : many}`
}

/** Splits a normalised pattern into text and `<placeholder>` parts for highlighting. */
export function patternParts(p: string): { text: string; v: boolean }[] {
  const out: { text: string; v: boolean }[] = []
  const re = /<(n|ip|uuid|email|hex|str)>/g
  let last = 0
  for (const m of p.matchAll(re)) {
    if (m.index! > last) out.push({ text: p.slice(last, m.index), v: false })
    out.push({ text: m[0], v: true })
    last = m.index! + m[0].length
  }
  if (last < p.length) out.push({ text: p.slice(last), v: false })
  return out
}

export function firstLine(s: string): string {
  const i = s.indexOf('\n')
  return i < 0 ? s : s.slice(0, i)
}

const TS = /^\s*(?:\[?\d{4}[-/]\d{2}[-/]\d{2}[T ]\d{2}:\d{2}:\d{2}(?:[.,]\d+)?(?:Z|[+-]\d{2}:?\d{2}|\s(?:UTC|GMT|[A-Z]{1,3}T)\b)?\]?|\[\d{2}\/\w{3}\/\d{4}:\d{2}:\d{2}:\d{2} [+-]\d{4}\]|\w{3} [ \d]\d \d{2}:\d{2}:\d{2}|\[?\d{2}:\d{2}:\d{2}(?:[.,]\d+)?\]?)\s*/
const PREFIX =
  /^(?:\[\d+\]\s+)?(?:\[(?:trace|debug|info|notice|warn|warning|error|err|fatal|crit|critical|alert|emerg|panic|TRACE|DEBUG|INFO|NOTICE|WARN|WARNING|ERROR|ERR|FATAL|CRIT|CRITICAL|ALERT|EMERG|PANIC)\]:?\s+|(?:TRACE|DEBUG|INFO|NOTICE|WARN|WARNING|ERROR|ERR|FATAL|CRITICAL|PANIC|LOG):?\s+)?(?:\d+#\d+:\s+(?:\*\d+\s+)?)?(?:\d+\s+---\s+(?:\[[^\]]*\]\s*)+)?/

/** A log line without the timestamp, level and thread the stream already shows. Mirrors `strip_prefix` in the core. */
export function tidy(line: string): string {
  const rest = line.replace(TS, '')
  const m = PREFIX.exec(rest)
  return m && m[0].length < rest.length ? rest.slice(m[0].length) : rest || line
}

/** Splits text into plain and highlighted parts for the search terms (lower case). */
export function marks(text: string, terms: string[]): { t: string; m: boolean }[] {
  if (!terms.length) return [{ t: text, m: false }]
  const lower = text.toLowerCase()
  const out: { t: string; m: boolean }[] = []
  let i = 0
  while (i < text.length) {
    let best = -1
    let len = 0
    for (const term of terms) {
      const j = lower.indexOf(term, i)
      if (j >= 0 && (best < 0 || j < best)) {
        best = j
        len = term.length
      }
    }
    if (best < 0) break
    if (best > i) out.push({ t: text.slice(i, best), m: false })
    out.push({ t: text.slice(best, best + len), m: true })
    i = best + len
  }
  if (i < text.length) out.push({ t: text.slice(i), m: false })
  return out
}

/** Free-text words of a search, without `key:value` filters. */
export function searchTerms(q: string): string[] {
  return q
    .split(/\s+/)
    .filter((t) => t && !/^[a-z]+:/i.test(t))
    .map((t) => t.toLowerCase())
}
