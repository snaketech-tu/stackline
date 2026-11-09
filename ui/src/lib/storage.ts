// Settings remembered in the browser. Storage can be blocked; then nothing is remembered.

/** Reads a remembered setting. Settings saved before the rename from swarmdash to Stackline are read under their old name. */
export function load(key: string): string | null {
  try {
    return localStorage.getItem(key) ?? localStorage.getItem(key.replace(/^stackline-/, 'swarmdash-'))
  } catch {
    return null
  }
}
export function save(key: string, v: string) {
  try {
    localStorage.setItem(key, v)
  } catch {
    // Storage blocked: the setting just isn't remembered.
  }
}
