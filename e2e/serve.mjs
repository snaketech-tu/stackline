// Starts Stackline for the UI tests: a fresh data folder, a fixed sign-in token,
// and a guard in front of the Docker socket that lets only reads through and
// writes down every call, so the tests can check that Stackline stayed read-only.
import { spawn } from 'node:child_process'
import { appendFileSync, mkdirSync, rmSync, writeFileSync } from 'node:fs'
import http from 'node:http'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const RUN = join(here, '.run')
const root = resolve(here, '..')
const bin = process.env.STACKLINE_BIN ?? join(root, 'target', 'debug', 'stackline')
const docker = process.env.DOCKER_SOCK ?? '/var/run/docker.sock'
const port = process.env.STACKLINE_PORT ?? '7171'

rmSync(RUN, { recursive: true, force: true })
mkdirSync(join(RUN, 'data'), { recursive: true })
const guard = join(RUN, 'docker.sock')
const calls = join(RUN, 'docker-calls.log')
writeFileSync(calls, '')

http
  .createServer((req, res) => {
    appendFileSync(calls, `${req.method} ${req.url}\n`)
    if (req.method !== 'GET' && req.method !== 'HEAD') {
      res.writeHead(403, { 'content-type': 'application/json' }).end('{"message":"refused by the Stackline test guard: not a read"}')
      return
    }
    const up = http.request({ socketPath: docker, method: req.method, path: req.url, headers: req.headers }, (r) => {
      res.writeHead(r.statusCode ?? 502, r.headers)
      r.pipe(res)
    })
    up.on('error', (e) => res.writeHead(502).end(String(e)))
    res.on('close', () => up.destroy())
    req.pipe(up)
  })
  .listen(guard)

writeFileSync(
  join(RUN, 'data', 'config.json'),
  JSON.stringify({
    connections: [
      { id: 'local', name: 'This computer', kind: 'local', endpoint: `unix://${guard}` },
      // Never reachable: only there so the connection menu has a second entry.
      { id: 'spare', name: 'Spare host', kind: 'local', endpoint: `unix://${join(RUN, 'nothing.sock')}` },
    ],
    settings: { checkUpdates: false, logPollSecs: 2, refreshSecs: 5 },
  }),
)

const child = spawn(bin, ['serve', '--no-open', '--port', port, '--data-dir', join(RUN, 'data')], {
  env: { ...process.env, STACKLINE_TOKEN: process.env.STACKLINE_TOKEN ?? '0123456789abcdef0123456789abcdef' },
  stdio: 'inherit',
})
child.on('exit', (code) => process.exit(code ?? 1))
for (const sig of ['SIGINT', 'SIGTERM']) process.on(sig, () => child.kill(sig))
