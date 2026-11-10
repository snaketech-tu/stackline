import { expect, test } from '@playwright/test'
import { TOKEN } from '../playwright.config'

// Needs the swarm stack of dev/demo/stack.yaml deployed as "shop": `docker
// swarm init`, then `docker stack deploy -c stack.yaml shop` in dev/demo. Run
// with `npx playwright test --project swarm`; CI does this in its own job,
// because the crashing service of the stack fills Docker's short event
// history and would push out the events the other tests look for.

const headers = { 'x-stackline-token': TOKEN }
const hour = () => {
  const to = Date.now() + 60e3
  return `from=${to - 3600e3}&to=${to}`
}

type Service = { fullName: string; mode: string; desired: number; running: number }

test('a swarm stack shows its services with their mode and replicas', async ({ request }) => {
  await expect(async () => {
    const { snapshot } = await (await request.get('/api/c/local/snapshot', { headers })).json()
    expect(snapshot.mode).toBe('swarm')
    const shop: Service[] = snapshot.stacks.find((s: { name: string }) => s.name === 'shop').services
    const by = Object.fromEntries(shop.map((s) => [s.fullName, s]))
    expect(by.shop_api).toMatchObject({ mode: 'replicated', desired: 3, running: 3 })
    expect(by.shop_agent).toMatchObject({ mode: 'global', running: 1 })
    expect(by.shop_broken).toMatchObject({ desired: 1, running: 0 })
  }).toPass({ timeout: 30_000 })
})

test('lines of every replica are stored, stack traces in one piece', async ({ request }) => {
  await expect(async () => {
    const r = await request.get(`/api/c/local/logs?service=shop_api&${hour()}&limit=500`, { headers })
    const { entries } = (await r.json()).result as { entries: { replica: string; text: string }[] }
    expect(new Set(entries.map((e) => e.replica))).toEqual(new Set(['1', '2', '3']))
    const trace = entries.find((e) => e.text.includes('NullPointerException'))
    expect(trace?.text).toMatch(/ERROR[\s\S]*\n\s+at com\.acme\./)
  }).toPass({ timeout: 60_000 })
})

test('a service that keeps failing has its crashes and exit code listed', async ({ request }) => {
  await expect(async () => {
    const r = await request.get(`/api/c/local/events?service=shop_broken&${hour()}&limit=100`, { headers })
    const { events } = await r.json()
    expect(events.some((e: { kind: string; exitCode: number | null }) => e.kind === 'crash' && e.exitCode === 1)).toBe(true)
  }).toPass({ timeout: 60_000 })
})
