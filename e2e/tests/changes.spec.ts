import { expect, test } from '@playwright/test'
import { TOKEN } from '../playwright.config'
import { inspect, open, serviceRow } from './helpers'

const headers = { 'x-stackline-token': TOKEN }

// Whatever a test leaves behind, the connection is read-only again for the next one.
test.afterEach(async ({ request }) => {
  await request.put('/api/c/local/access', { headers, data: { writable: false } })
})

test('a connection refuses changes until read-only is switched off', async ({ request }) => {
  const snap = await (await request.get('/api/c/local/snapshot', { headers })).json()
  expect(snap.access).toEqual({ writable: false, forced: false })
  const r = await request.post('/api/c/local/action', { headers, data: { action: 'restart', service: 'demo_shop', dry: true } })
  expect(r.status()).toBe(403)
  expect((await r.json()).error).toContain('read-only')
})

test('switching read-only off asks first, and every change shows its docker command before it runs', async ({ page }) => {
  await open(page, 'services')
  const badge = page.locator('header.top').getByRole('switch', { name: /Read-only/ })
  await expect(badge).toHaveAttribute('aria-checked', 'true')
  const bar = page.getByLabel('Actions')
  await serviceRow(page, 'demo/shop').click()
  // While read-only, the lists offer no changes.
  await expect(bar.getByRole('button', { name: /Restart/ })).toHaveCount(0)

  await badge.click()
  const ask = page.getByRole('alertdialog')
  await expect(ask).toContainText('Allow changes on This computer?')
  // Cancelled: still read-only.
  await page.keyboard.press('Escape')
  await expect(ask).toHaveCount(0)
  await expect(badge).toHaveAttribute('aria-checked', 'true')

  await badge.click()
  await ask.getByRole('button', { name: 'Allow changes' }).click()
  const open_ = page.locator('header.top').getByRole('switch', { name: /Changes allowed/ })
  await expect(open_).toHaveAttribute('aria-checked', 'false')
  await expect(page.locator('.rl-foot')).toContainText('each one asks first')

  // `r` on the row asks to restart, naming the containers like `docker restart` would.
  await page.locator('main.view').click({ position: { x: 5, y: 5 } })
  await page.keyboard.press('r')
  await expect(ask).toContainText('Restart demo_shop?')
  await expect(ask.locator('code')).toHaveText(/^docker restart \S+shop\S* \S+shop\S*$/)
  await ask.getByRole('button', { name: 'Cancel' }).click()
  await expect(ask).toHaveCount(0)

  // Stop on one container from the containers list.
  await serviceRow(page, 'demo/shop').dblclick()
  await expect(page).toHaveURL(/#tasks\?service=demo%2Fshop$/)
  await page.getByRole('tab', { name: /^Running/ }).click()
  await page.keyboard.press('j')
  await bar.getByRole('button', { name: /Stop/ }).click()
  await expect(ask.locator('code')).toHaveText(/^docker stop \S+$/)
  await page.keyboard.press('Escape')
  await expect(ask).toHaveCount(0)

  // Back to read-only with one click; the changes are gone from the lists.
  await open_.click()
  await expect(badge).toHaveAttribute('aria-checked', 'true')
  await expect(bar.getByRole('button', { name: /Stop/ })).toHaveCount(0)
})

test('the inspector offers the changes too, while they are allowed', async ({ page, request }) => {
  await request.put('/api/c/local/access', { headers, data: { writable: true } })
  const panel = await inspect(page, 'demo/worker')
  await expect(panel.getByRole('group', { name: 'Change this service' })).toBeVisible()
  await panel.getByRole('button', { name: 'Restart' }).click()
  await expect(page.getByRole('alertdialog').locator('code')).toContainText('docker restart')
  await page.keyboard.press('Escape')
  await expect(page.getByRole('alertdialog')).toHaveCount(0)
  // Escape closed only the dialog, not the inspector.
  await expect(panel).toBeVisible()
})

test('the palette finds "allow changes", and the dialog keeps its keys after a click inside it', async ({ page }) => {
  await open(page, 'services')
  await page.keyboard.press(':')
  await page.keyboard.type('allow changes')
  await page.keyboard.press('Enter')
  const ask = page.getByRole('alertdialog')
  // The Enter that ran the command does not also confirm the dialog.
  await expect(ask).toContainText('Allow changes on This computer?')
  await expect(page.locator('header.top').getByRole('switch')).toHaveAttribute('aria-checked', 'true')
  // A click on its text takes focus off the buttons; Escape still closes it.
  await ask.locator('p').click()
  await page.keyboard.press('Escape')
  await expect(ask).toHaveCount(0)
  await expect(page.locator('header.top').getByRole('switch')).toHaveAttribute('aria-checked', 'true')
})

test('a change runs only as confirmed', async ({ request }) => {
  await request.put('/api/c/local/access', { headers, data: { writable: true } })
  const ask = { action: 'restart', service: 'demo_worker' }
  const dry = await (await request.post('/api/c/local/action', { headers, data: { ...ask, dry: true } })).json()
  expect(dry.command).toContain('docker restart')
  expect(dry.plan).toBeTruthy()
  // A plan made from another state of the service is refused before anything is sent.
  const stale = await request.post('/api/c/local/action', { headers, data: { ...ask, expect: 'not-this-plan' } })
  expect(stale.status()).toBe(409)
})
