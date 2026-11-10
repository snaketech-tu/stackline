// Time ranges for the Logs page: typed ones ("2h", "yesterday", "14:00-15:30"),
// relative ones that end now, and how a range reads on screen. Local time throughout.

/** A range that ends now and moves with it: `auto` or a length such as `15m`, `6h`, `2d`. */
export type Preset = string

/**
 * What a typed range means. `preset` moves with the clock. A `from` with
 * `to: null` starts at a fixed time and runs until now (open end).
 */
export type Range = { preset: Preset } | { from: number; to: number | null }

const UNIT_MS = { s: 1e3, m: 60e3, h: 3600e3, d: 86400e3, w: 7 * 86400e3 } as const
type Unit = keyof typeof UNIT_MS

const UNITS: [RegExp, Unit][] = [
  [/^(s|secs?|seconds?)$/, 's'],
  [/^(m|mins?|minutes?|minuten?)$/, 'm'],
  [/^(h|hrs?|hours?|std|stunden?)$/, 'h'],
  [/^(d|days?|tage?n?)$/, 'd'],
  [/^(w|wks?|weeks?|wochen?)$/, 'w'],
]

/** `90 min`, `2h`, `1.5 days` as milliseconds, or null. */
export function duration(text: string): number | null {
  const m = /^(\d+(?:[.,]\d+)?)\s*([a-zäöü]+)$/i.exec(text.trim())
  if (!m) return null
  const unit = UNITS.find(([re]) => re.test(m[2].toLowerCase()))?.[1]
  const n = parseFloat(m[1].replace(',', '.'))
  if (!unit || !(n > 0)) return null
  return n * UNIT_MS[unit]
}

/** The length of a preset in ms; `auto` and unknown presets have none. */
export function presetMs(p: Preset): number | null {
  const m = /^(\d+)([mhd])$/.exec(p)
  return m ? +m[1] * UNIT_MS[m[2] as Unit] : null
}

/** The shortest preset name for a length: 90 minutes is `90m`, 48 hours `2d`. */
export function presetFor(ms: number): Preset {
  const mins = Math.max(1, Math.round(ms / 60e3))
  if (mins % 1440 === 0 && mins >= 2880) return `${mins / 1440}d`
  if (mins % 60 === 0) return `${mins / 60}h`
  return `${mins}m`
}

function midnight(t: number, addDays = 0): number {
  const d = new Date(t)
  d.setHours(0, 0, 0, 0)
  d.setDate(d.getDate() + addDays)
  return d.getTime()
}

/** Monday 00:00 of the week of `t`. */
function weekStart(t: number): number {
  const d = new Date(midnight(t))
  return midnight(d.getTime(), -((d.getDay() + 6) % 7))
}

function at(day: number, h: number, mi: number, s = 0): number {
  const d = new Date(day)
  d.setHours(h, mi, s, 0)
  return d.getTime()
}

/** A clock time `14:00`, `9:30:15` or `14h`. */
function clock(text: string): [number, number, number] | null {
  const m = /^(\d{1,2})(?::(\d{2}))?(?::(\d{2}))?h?$/.exec(text)
  // A bare number is a time only as `14h`; `14` alone is too vague.
  if (!m || (m[2] === undefined && !text.endsWith('h'))) return null
  const [h, mi, s] = [+m[1], +(m[2] ?? 0), +(m[3] ?? 0)]
  return h < 24 && mi < 60 && s < 60 ? [h, mi, s] : null
}

const WEEKDAYS = [
  ['sun', 'sunday', 'so', 'sonntag'],
  ['mon', 'monday', 'mo', 'montag'],
  ['tue', 'tuesday', 'di', 'dienstag'],
  ['wed', 'wednesday', 'mi', 'mittwoch'],
  ['thu', 'thursday', 'do', 'donnerstag'],
  ['fri', 'friday', 'fr', 'freitag'],
  ['sat', 'saturday', 'sa', 'samstag'],
]

/**
 * A day: `today`, `yesterday`, a weekday (the last one, today included),
 * `2026-11-09`, `9.11.2026`, `9.11.` or `11/9`. Gives midnight of that day.
 */
function day(text: string, now: number): number | null {
  const t = text.toLowerCase()
  if (t === 'today' || t === 'heute') return midnight(now)
  if (t === 'yesterday' || t === 'gestern') return midnight(now, -1)
  const wd = WEEKDAYS.findIndex((names) => names.includes(t))
  if (wd >= 0) return midnight(now, -((new Date(now).getDay() - wd + 7) % 7))
  let y: number, mo: number, d: number
  let m: RegExpExecArray | null
  if ((m = /^(\d{4})-(\d{1,2})-(\d{1,2})$/.exec(t))) [y, mo, d] = [+m[1], +m[2], +m[3]]
  else if ((m = /^(\d{1,2})\.(\d{1,2})\.(\d{4}|\d{2})?$/.exec(t))) [d, mo, y] = [+m[1], +m[2], m[3] ? (m[3].length === 2 ? 2000 + +m[3] : +m[3]) : NaN]
  else if ((m = /^(\d{1,2})\/(\d{1,2})(?:\/(\d{4}))?$/.exec(t))) [mo, d, y] = [+m[1], +m[2], m[3] ? +m[3] : NaN]
  else return null
  const year = Number.isNaN(y) ? new Date(now).getFullYear() : y
  const date = new Date(year, mo - 1, d)
  if (date.getMonth() !== mo - 1 || date.getDate() !== d) return null
  // A day without a year is the last one that has passed.
  if (Number.isNaN(y) && date.getTime() > now) date.setFullYear(year - 1)
  return date.getTime()
}

/** One point in time, or null. `day` is set when only a day was given (no clock time). */
function point(text: string, now: number): { t: number; day?: boolean } | null {
  const s = text.trim().toLowerCase().replace(/\s+/g, ' ')
  if (s === 'now' || s === 'jetzt') return { t: now }
  let m: RegExpExecArray | null
  // now-2h, now - 30m
  // now-2h, 2h ago, vor 2h: to the minute, so the range reads and links cleanly
  if ((m = /^now ?- ?(.+)$/.exec(s)) || (m = /^(.+?) ago$/.exec(s)) || (m = /^vor (.+)$/.exec(s))) {
    const d = duration(m[1])
    return d ? { t: Math.floor((now - d) / 60e3) * 60e3 } : null
  }
  // 2026-11-09T14:00 from a date field or an ISO time
  if ((m = /^(\d{4}-\d{1,2}-\d{1,2})t(\d{1,2}:\d{2}(?::\d{2})?)(?:\.\d+)?$/.exec(s))) return point(`${m[1]} ${m[2]}`, now)
  const c = clock(s)
  if (c) {
    // A time without a day is the last time the clock showed it.
    const today = at(midnight(now), ...c)
    return { t: today <= now ? today : at(midnight(now, -1), ...c) }
  }
  const dd = day(s, now)
  if (dd !== null) return { t: dd, day: true }
  // day + time, in either order: `yesterday 14:00`, `9.11. 8:30`, `14:00 yesterday`
  const parts = s.split(' ')
  if (parts.length >= 2) {
    for (const [dt, tm] of [[parts.slice(0, -1).join(' '), parts.at(-1)!], [parts.slice(1).join(' '), parts[0]]]) {
      const base = day(dt, now)
      const cl = clock(tm.replace(/^(um|at)$/, ''))
      if (base !== null && cl) return { t: at(base, ...cl) }
    }
    // `yesterday at 14:00`, `gestern um 14:00`
    if (parts.length === 3 && /^(at|um)$/.test(parts[1])) return point(`${parts[0]} ${parts[2]}`, now)
  }
  return null
}

/**
 * Reads a typed range. Understands, among others:
 * `15m`, `last 2 hours`, `today`, `yesterday`, `this week`, `since 14:00`,
 * `14:00-15:30`, `yesterday 22:00 to today 02:00`, `2026-11-09`,
 * `9.11. 8:00 - 9:00`, `14:05 +/- 5m`, `14:00 +30m`, `now-6h to now-2h`, `auto`.
 */
export function parseRange(text: string, now = Date.now()): Range | null {
  let s = text.trim().toLowerCase().replace(/\s+/g, ' ')
  if (!s) return null
  if (s === 'auto' || s === 'all' || s === 'everything' || s === 'alles') return { preset: 'auto' }
  s = s.replace(/^(the )?(last|past|letzte[nr]?) /, '')
  const len = duration(s)
  if (len) return { preset: presetFor(len) }
  if (s === 'hour' || s === 'stunde') return { preset: '1h' }
  if (s === 'day' || s === 'tag') return { preset: '24h' }
  if (s === 'week' || s === 'woche') return { preset: '7d' }
  if (s === 'today' || s === 'heute') return { from: midnight(now), to: null }
  if (s === 'this week' || s === 'diese woche') return { from: weekStart(now), to: null }
  if (s === 'previous week' || s === 'vorige woche') return { from: weekStart(now) - 7 * 86400e3, to: weekStart(now) }

  let m: RegExpExecArray | null
  // since 14:00, from yesterday, after 9.11.
  if ((m = /^(since|from|after|seit|ab) (.+)$/.exec(s)) && !/ (-|–|to|until|till|bis) /.test(m[2])) {
    const p = point(m[2], now)
    return p && p.t < now ? { from: p.t, to: null } : null
  }
  // before 14:00, until 14:00: the hour leading up to it
  if ((m = /^(before|until|till|bis) (.+)$/.exec(s))) {
    const p = point(m[2], now)
    return p ? { from: p.t - 3600e3, to: p.t } : null
  }
  // around 14:05, 14:05 +/- 10m, 14:05 ± 10m
  if ((m = /^(?:around|um|ca\.?) (.+?)(?: ?(?:\+\/?-|±) ?(.+))?$/.exec(s)) || (m = /^(.+?) ?(?:\+\/?-|±) ?(.+)$/.exec(s))) {
    const p = point(m[1], now)
    const d = m[2] ? duration(m[2]) : 5 * 60e3
    return p && d ? { from: p.t - d, to: p.t + d } : null
  }
  // 14:00 +30m: from a time, for a while
  if ((m = /^(.+?) ?\+ ?(\d.*)$/.exec(s))) {
    const p = point(m[1], now)
    const d = duration(m[2])
    return p && d ? { from: p.t, to: p.t + d } : null
  }
  // A from and a to: `a - b`, `a to b`, `a..b`, `14:00-15:30`
  const split =
    /^(.+?) (?:-|–|—|to|until|till|bis) (.+)$/.exec(s) ??
    /^(.+?) ?\.\. ?(.+)$/.exec(s) ??
    /^(\d{1,2}(?::\d{2}){0,2})[-–](\d{1,2}(?::\d{2}){0,2})$/.exec(s) ??
    /^(.*\d{1,2}:\d{2}(?::\d{2})?)[-–](\d{1,2}:\d{2}(?::\d{2})?)$/.exec(s)
  if (split) {
    const a = point(split[1], now)
    if (!a) return null
    const bText = split[2]
    if (/^(now|jetzt)$/.test(bText)) return a.t < now ? { from: a.t, to: null } : null
    // The end may be just a clock time: on the day the range starts, or the next.
    const cl = clock(bText)
    let b: number | null
    if (cl) {
      b = at(midnight(a.t), ...cl)
      if (b <= a.t) b = at(midnight(a.t, 1), ...cl)
    } else {
      const p = point(bText, now)
      // A day as the end includes the whole day.
      b = p ? (p.day ? midnight(p.t, 1) : p.t) : null
    }
    return b !== null && b > a.t ? { from: a.t, to: b } : null
  }
  // A single day is the whole day; a single time is "since".
  const p = point(s, now)
  if (!p) return null
  if (p.day) return { from: p.t, to: midnight(p.t, 1) }
  return p.t < now ? { from: p.t, to: null } : null
}

const pad = (n: number) => String(n).padStart(2, '0')

/** `14:05`, or `14:05:30` with `seconds`. */
function clockText(t: number, seconds = false): string {
  const d = new Date(t)
  const s = `${pad(d.getHours())}:${pad(d.getMinutes())}`
  return seconds ? `${s}:${pad(d.getSeconds())}` : s
}

/** `today`, `yesterday`, `Mon 9 Nov`, or `2025-11-09` in another year. */
function dayText(t: number, now: number): string {
  if (t === midnight(now)) return 'today'
  if (t === midnight(now, -1)) return 'yesterday'
  const d = new Date(t)
  if (d.getFullYear() === new Date(now).getFullYear()) return d.toLocaleDateString('en-GB', { weekday: 'short', day: 'numeric', month: 'short' })
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`
}

/** `14:05` today, `yesterday 14:05`, `Mon 9 Nov 14:05` this year, else `2025-11-09 14:05`. */
export function pointText(t: number, now = Date.now(), seconds = false): string {
  const d = new Date(t)
  const c = clockText(t, seconds)
  const m0 = midnight(now)
  if (t >= m0 && t < midnight(now, 1)) return c
  if (t >= midnight(now, -1) && t < m0) return `yesterday ${c}`
  if (d.getFullYear() === new Date(now).getFullYear())
    return `${d.toLocaleDateString('en-GB', { weekday: 'short', day: 'numeric', month: 'short' })} ${c}`
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${c}`
}

/** How a fixed range reads: `14:00 – 15:30`, `yesterday 22:00 – 02:00`, `Mon 9 Nov 08:00 – 12:00`. */
export function rangeText(from: number, to: number | null, now = Date.now()): string {
  if (to === null) return from === midnight(now) ? 'today' : `since ${pointText(from, now)}`
  // Whole days read as days: `yesterday`, `Mon 9 Nov – today`.
  if (from === midnight(from) && to === midnight(to)) {
    const a = dayText(from, now)
    return to === midnight(from, 1) ? a : `${a} – ${dayText(midnight(to, -1), now)}`
  }
  const seconds = to - from < 10 * 60e3 && (new Date(from).getSeconds() !== 0 || new Date(to).getSeconds() !== 0)
  const a = pointText(from, now, seconds)
  // The end on the same day or the night after reads as a clock time only.
  const short = midnight(to) === midnight(from) || (to - from < 86400e3 && midnight(to) === midnight(from, 1) && to < midnight(now, 1))
  const b = short ? clockText(to, seconds) : pointText(to, now, seconds)
  return `${a} – ${b}`
}

/** A preset as words: `last 15 minutes`, `last 6 hours`, `last 2 days`. */
export function presetText(p: Preset): string {
  const m = /^(\d+)([mhd])$/.exec(p)
  if (!m) return p
  const n = +m[1]
  const word = { m: 'minute', h: 'hour', d: 'day' }[m[2] as 'm' | 'h' | 'd']
  return n === 1 ? `last ${word}` : `last ${n} ${word}s`
}

/** The value for an `<input type="datetime-local">`. */
export function inputValue(t: number): string {
  const d = new Date(t)
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`
}

/** A range as text `parseRange` reads back: `2026-11-09 14:00 - 2026-11-09 15:30`, `since 2026-11-09 14:00`. */
export function rangeInput(from: number, to: number | null): string {
  const t = (x: number) => inputValue(x).replace('T', ' ')
  return to === null ? `since ${t(from)}` : `${t(from)} - ${t(to)}`
}
