// Derive the logo variants and app icon from the editable SVG masters.
// Run from any directory: node dev/generate-branding.mjs
import { readFileSync, writeFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

const root = new URL('../', import.meta.url)
const read = (path) => readFileSync(new URL(path, root), 'utf8')
const write = (path, contents) => writeFileSync(new URL(path, root), contents)
const body = (svg) => svg.replace(/^[\s\S]*?<svg[^>]*>/, '').replace(/<\/svg>\s*$/, '').replace(/<title[^>]*>[\s\S]*?<\/title>/g, '').trim()
const mark = body(read('ui/public/brand/stackline-mark.svg'))
const wordmarkSvg = read('ui/public/brand/stackline-wordmark.svg')
const wordmark = body(wordmarkSvg)
const [, wordWidth, wordHeight] = wordmarkSvg.match(/viewBox="0 0 ([\d.]+) ([\d.]+)"/)
const scale = 60 / Number(wordHeight)
const width = Math.ceil(144 + Number(wordWidth) * scale + 16)

for (const [theme, accent, ink] of [['light', '#0d8a86', '#0a0f13'], ['dark', '#2fd0bd', '#e4ecf1']]) {
  write(`ui/public/brand/stackline-${theme}.svg`, `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${width} 112" role="img" aria-labelledby="logo-title">
  <title id="logo-title">swarmdash</title>
  <g transform="translate(16 16)" color="${accent}">${mark}</g>
  <g transform="translate(144 26) scale(${scale.toFixed(8)})" color="${ink}">${wordmark}</g>
</svg>
`)
}

const icon = (size) => `<svg xmlns="http://www.w3.org/2000/svg" width="${size}" height="${size}" viewBox="0 0 128 128" role="img" aria-labelledby="icon-title">
  <title id="icon-title">swarmdash</title>
  <rect x="4" y="4" width="120" height="120" rx="28" fill="#0d8a86"/>
  <g transform="translate(24 30.666667) scale(0.83333333)" color="#fff">${mark}</g>
</svg>
`
write('desktop/icon.svg', icon(1024))
write('ui/public/favicon.svg', icon(128))
console.log(`Generated Stackline logos and icon sources in ${fileURLToPath(root)}`)
