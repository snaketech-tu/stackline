// A small Markdown renderer for release notes. Everything is escaped first,
// so only the markup produced here reaches the page.

const esc = (s: string) => s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]!)

function inline(s: string): string {
  const codes: string[] = []
  let out = esc(s).replace(/`([^`]+)`/g, (_, c) => {
    codes.push(`<code>${c}</code>`)
    return `\u0000${codes.length - 1}\u0000`
  })
  out = out
    .replace(/\*\*(.+?)\*\*/g, '<b>$1</b>')
    .replace(/(^|[\s(])_(.+?)_(?=[\s).,!?:;]|$)/g, '$1<i>$2</i>')
    .replace(/\[([^\]]+)\]\((https?:\/\/[^)\s]+)\)/g, '<a href="$2" target="_blank" rel="noreferrer">$1</a>')
    .replace(/(^|\s)(https?:\/\/[^\s<]+[^\s<.,)])/g, '$1<a href="$2" target="_blank" rel="noreferrer">$2</a>')
  return out.replace(/\u0000(\d+)\u0000/g, (_, i) => codes[+i])
}

export function markdown(src: string): string {
  const lines = src.replace(/\r\n?/g, '\n').split('\n')
  const html: string[] = []
  let list: 'ul' | 'ol' | null = null
  let para: string[] = []
  const flush = () => {
    if (para.length) html.push(`<p>${inline(para.join(' '))}</p>`)
    para = []
  }
  const close = () => {
    if (list) html.push(`</${list}>`)
    list = null
  }
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i]
    if (line.startsWith('```')) {
      flush()
      close()
      const code: string[] = []
      while (++i < lines.length && !lines[i].startsWith('```')) code.push(lines[i])
      html.push(`<pre>${esc(code.join('\n'))}</pre>`)
      continue
    }
    const h = line.match(/^(#{1,6})\s+(.*)$/)
    const li = line.match(/^\s*[-*+]\s+(.*)$/)
    const ol = line.match(/^\s*\d+[.)]\s+(.*)$/)
    if (h) {
      flush()
      close()
      const level = Math.min(6, h[1].length + 2)
      html.push(`<h${level}>${inline(h[2])}</h${level}>`)
    } else if (li || ol) {
      flush()
      const kind = li ? 'ul' : 'ol'
      if (list !== kind) {
        close()
        html.push(`<${kind}>`)
        list = kind
      }
      html.push(`<li>${inline((li ?? ol)![1])}</li>`)
    } else if (!line.trim()) {
      flush()
      close()
    } else if (list && /^\s{2,}\S/.test(line)) {
      // Continuation of the previous list item.
      html[html.length - 1] = html[html.length - 1].replace(/<\/li>$/, ' ' + inline(line.trim()) + '</li>')
    } else {
      close()
      para.push(line.trim())
    }
  }
  flush()
  close()
  return html.join('\n')
}
