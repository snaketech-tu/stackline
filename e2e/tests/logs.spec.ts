import { expect, test } from '@playwright/test'
import { nav, open, serviceRow } from './helpers'

test('a multi-line stack trace is one entry', async ({ page }) => {
  await open(page, 'logs')
  await page.getByPlaceholder(/^Search/).fill('Checkout failed')
  const row = page.locator('.stream .row', { hasText: 'Checkout failed' }).first()
  await expect(row).toContainText('lines')
  await row.click()
  await expect(row.locator('pre.trace')).toContainText('Caused by: java.lang.IllegalArgumentException')
  await expect(row.locator('pre.trace')).toContainText('at com.acme.shop.checkout.CheckoutController.preview')
})

test('log filters stay when you go to another page and back', async ({ page }) => {
  await open(page, 'logs')
  const levels = page.getByRole('group', { name: 'Levels' })
  await levels.getByRole('button', { name: /^Error/ }).click()
  await page.getByPlaceholder(/^Search/).fill('cart')
  await expect(levels.getByRole('button', { name: /^Warn/ })).toHaveAttribute('aria-pressed', 'false')

  await nav(page, 'Overview')
  await expect(page.locator('.kpi').first()).toBeVisible()
  await nav(page, /^Logs/)

  await expect(levels.getByRole('button', { name: /^Error/ })).toHaveAttribute('aria-pressed', 'true')
  await expect(levels.getByRole('button', { name: /^Warn/ })).toHaveAttribute('aria-pressed', 'false')
  await expect(page.getByPlaceholder(/^Search/)).toHaveValue('cart')
  await expect(page.locator('.stream .row').first()).toContainText('cart')
})

test('the source picked in Logs is kept after a reload', async ({ page }) => {
  await open(page, 'services')
  await serviceRow(page, 'demo/worker').click()
  // `o` opens the lines of the row on the Logs page.
  await page.keyboard.press('o')
  await expect(page).toHaveURL(/#logs\?.*s=demo_worker/)
  const rows = page.locator('.stream .row .svc')
  await expect(rows.first()).toContainText('worker')
  await page.reload()
  await expect(rows.first()).toContainText('worker')
  expect(await rows.allTextContents()).not.toContainEqual(expect.stringMatching(/shop|app|caddy/))
})

test('the list filters stay when you go to another page and back', async ({ page }) => {
  await open(page, 'services')
  await page.getByPlaceholder('Filter by name, image, stack or node').fill('shop')
  await expect(page.getByRole('row', { name: /worker/ })).toHaveCount(0)
  await nav(page, 'Overview')
  await nav(page, /^Services/)
  await expect(page.getByPlaceholder('Filter by name, image, stack or node')).toHaveValue('shop')
  await expect(page.getByRole('row', { name: /worker/ })).toHaveCount(0)
  await page.reload()
  await expect(page.getByPlaceholder('Filter by name, image, stack or node')).toHaveValue('shop')
  await page.getByPlaceholder('Filter by name, image, stack or node').fill('')
})

test('the address of the Logs page opens the same view in another tab', async ({ page, context }) => {
  await open(page, 'logs')
  await page.getByRole('group', { name: 'Time range' }).getByRole('button', { name: '1h' }).click()
  await page.getByRole('group', { name: 'Levels' }).getByRole('button', { name: /^Error/ }).click()
  await page.getByPlaceholder(/^Search/).fill('cart')
  await expect(page).toHaveURL(/q=cart/)
  expect(page.url()).toMatch(/[#&?]t=1h(&|$)/)
  expect(page.url()).toMatch(/l=ERROR(&|$)/)

  const other = await context.newPage()
  await other.goto(page.url())
  const levels = other.getByRole('group', { name: 'Levels' })
  await expect(levels.getByRole('button', { name: /^Error/ })).toHaveAttribute('aria-pressed', 'true')
  await expect(levels.getByRole('button', { name: /^Warn/ })).toHaveAttribute('aria-pressed', 'false')
  await expect(other.getByRole('group', { name: 'Time range' }).getByRole('button', { name: '1h' })).toHaveAttribute('aria-pressed', 'true')
  await expect(other.getByPlaceholder(/^Search/)).toHaveValue('cart')
  await expect(other.locator('.stream .row').first()).toContainText('cart')
  await other.close()
})

test('Copy link fixes the time range, and the link opens from Ctrl K', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write'])
  await open(page, 'logs')
  await page.getByRole('group', { name: 'Levels' }).getByRole('button', { name: /^Warn/ }).click()
  await page.getByPlaceholder(/^Search/).fill('worker')
  await expect(page).toHaveURL(/q=worker/)
  await page.getByRole('button', { name: 'Copy link' }).click()
  const link = await page.evaluate(() => navigator.clipboard.readText())
  // A fixed range, so the link shows the same lines tomorrow.
  expect(link).toMatch(/#logs\?.*t=\d+-\d+/)
  expect(link).toContain('l=WARN')

  await page.getByRole('button', { name: 'Clear all' }).click()
  await nav(page, 'Overview')
  await page.keyboard.press('Control+k')
  await page.keyboard.insertText(link)
  await expect(page.getByText('Open this log link')).toBeVisible()
  await page.keyboard.press('Enter')
  await expect(page.getByRole('heading', { name: 'Log stream' })).toBeVisible()
  await expect(page.getByPlaceholder(/^Search/)).toHaveValue('worker')
  await expect(page.getByRole('group', { name: 'Levels' }).getByRole('button', { name: /^Warn/ })).toHaveAttribute('aria-pressed', 'true')
  await expect(page.getByRole('button', { name: 'Reset zoom' })).toBeVisible()
})

// `migrate` prints a 40 KB line, 1500 short lines and "Applied 3 migrations", then exits.
test('the stream pages through every stored line, down to the oldest', async ({ page }) => {
  await open(page, 'logs?c=local&s=demo_migrate')
  const rows = page.locator('.stream > .row')
  await expect(rows.first()).toContainText('Applied 3 migrations')
  const end = page.locator('.stream-end')
  await expect(end).toContainText(/Show 500 older lines|Loading older lines/)
  // Scrolling to the end of the stream brings in the older lines, page by page.
  await expect(async () => {
    await end.scrollIntoViewIfNeeded()
    await expect(end).toContainText('These are all the stored lines', { timeout: 1000 })
  }).toPass({ timeout: 30_000 })
  await expect(rows).toHaveCount(1502)
  await expect(rows.last()).toContainText('Schema dump')
  // Every line once, in order.
  const msgs = await rows.locator('.msg').allTextContents()
  const nums = msgs.map((m) => /Migrated row (\d+)/.exec(m)?.[1]).filter(Boolean).map(Number)
  expect(nums).toEqual(Array.from({ length: 1500 }, (_, i) => 1500 - i))

  // Other filters start over at the newest page, and paging works again.
  await page.getByRole('group', { name: 'Levels' }).getByRole('button', { name: /^Info/ }).click()
  await expect(async () => {
    await end.scrollIntoViewIfNeeded()
    await expect(end).toContainText('These are all the stored lines', { timeout: 1000 })
  }).toPass({ timeout: 30_000 })
  await expect(rows).toHaveCount(1502)
})

test('a line longer than 16 KB is one entry, whole, and the row says it goes on', async ({ page }) => {
  await open(page, 'logs?c=local&s=demo_migrate')
  await page.getByPlaceholder(/^Search/).fill('Schema dump')
  const row = page.locator('.stream > .row', { hasText: 'Schema dump' })
  await expect(row).toHaveCount(1)
  await expect(row.locator('.rep')).toContainText(/\+[\d,.]+ chars/)
  // The badge is on screen, not pushed out of the row by the long text.
  const [badge, box] = [await row.locator('.rep').boundingBox(), await row.boundingBox()]
  expect(badge!.x + badge!.width).toBeLessThanOrEqual(box!.x + box!.width)
  await row.click()
  await expect(row.locator('pre.trace')).toHaveText('Schema dump ' + 'x'.repeat(40000))
})
