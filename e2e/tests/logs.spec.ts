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
  await expect(page.getByRole('button', { name: 'Reset range' })).toBeVisible()
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

test('a typed time range shows those lines, and the arrows step through time', async ({ page }) => {
  await open(page, 'logs')
  const rangeButton = page.getByRole('button', { name: /Custom/ })
  await rangeButton.click()
  const typed = page.getByRole('textbox', { name: 'Type a time range' })
  await typed.fill('90 min')
  await expect(page.getByRole('button', { name: 'Show last 90 minutes' })).toBeVisible()
  await typed.press('Enter')
  await expect(page).toHaveURL(/[#&?]t=90m(&|$)/)
  await expect(page.getByRole('button', { name: /last 90 minutes/ }).first()).toBeVisible()
  await expect(page.locator('.stream .row').first()).toBeVisible()

  // Earlier: the 90 minutes before, a fixed range in the address.
  await page.getByRole('button', { name: 'Earlier' }).click()
  await expect(page).toHaveURL(/[#&?]t=\d+-\d+(&|$)/)
  const [a, z] = /t=(\d+)-(\d+)/.exec(page.url())!.slice(1).map(Number)
  expect(z - a).toBe(90 * 60e3)
  // Later again: back to a range that ends now.
  await page.getByRole('button', { name: 'Later' }).click()
  await expect(page).toHaveURL(/[#&?]t=90m(&|$)/)

  // A range that runs until now follows the clock and comes back from its link.
  await page.keyboard.press('t')
  await typed.fill('since 5h ago')
  await typed.press('Enter')
  await expect(page).toHaveURL(/[#&?]t=\d+-now(&|$)/)
  await expect(page.locator('.stream .row').first()).toBeVisible()
  await page.reload()
  await expect(page.getByRole('button', { name: /^since / }).first()).toBeVisible()

  // Typed ranges are offered again.
  await page.keyboard.press('t')
  await expect(page.getByRole('dialog', { name: 'Time range' })).toContainText('since 5h ago')
  await expect(page.getByRole('dialog', { name: 'Time range' })).toContainText('90 min')
  await page.keyboard.press('Escape')
})

test.describe('From and To', () => {
  // A 24-hour locale and a fixed zone, so typed times mean the same everywhere.
  test.use({ locale: 'de-DE', timezoneId: 'Europe/Berlin' })

  /** `dd.mm. HH:MM` of a moment `minsAgo` back, in the page's zone, and that moment to the minute. */
  async function moment(page: import('@playwright/test').Page, minsAgo: number) {
    return page.evaluate((m) => {
      const d = new Date(Math.floor((Date.now() - m * 60e3) / 60e3) * 60e3)
      const p = (n: number) => String(n).padStart(2, '0')
      return { text: `${p(d.getDate())}.${p(d.getMonth() + 1)}. ${p(d.getHours())}:${p(d.getMinutes())}`, t: d.getTime() }
    }, minsAgo)
  }

  test('typed From and To times stay while the page refreshes, and Enter shows exactly that range', async ({ page }) => {
    await open(page, 'logs')
    await page.keyboard.press('t')
    const dialog = page.getByRole('dialog', { name: 'Time range' })
    const from = dialog.getByRole('textbox', { name: 'From', exact: true })
    const to = dialog.getByRole('textbox', { name: 'To', exact: true })
    const a = await moment(page, 120)
    const z = await moment(page, 30)

    // Typed key by key, the way a person does it.
    await from.click()
    await page.keyboard.press('Control+a')
    await page.keyboard.type(a.text)
    await to.click()
    await page.keyboard.press('Control+a')
    await page.keyboard.type(z.text)
    await expect(dialog).not.toContainText('Not a time yet')

    // The page reloads its lines and the store meanwhile; what was typed must stay.
    await page.waitForTimeout(16_000)
    await expect(from).toHaveValue(a.text)
    await expect(to).toHaveValue(z.text)

    await to.press('Enter')
    await expect(dialog).toHaveCount(0)
    await expect(page).toHaveURL(new RegExp(`[#&?]t=${a.t}-${z.t}(&|$)`))
    await expect(page.locator('.stream .row').first()).toBeVisible()
  })

  test('an empty To runs until now, arrows nudge a time, and nonsense is refused', async ({ page }) => {
    await open(page, 'logs')
    await page.getByRole('button', { name: /Custom/ }).click()
    const dialog = page.getByRole('dialog', { name: 'Time range' })
    const from = dialog.getByRole('textbox', { name: 'From', exact: true })
    const to = dialog.getByRole('textbox', { name: 'To', exact: true })
    await expect(to).toHaveValue('')
    await expect(dialog).toContainText('Empty: until now')

    await from.fill('soon-ish')
    await expect(dialog).toContainText('Not a time yet')
    await dialog.getByRole('button', { name: 'Show this range' }).click()
    await expect(dialog.getByRole('alert')).toContainText('The start is not a time')

    const a = await moment(page, 90)
    await from.fill(a.text)
    await to.fill((await moment(page, 100)).text)
    await from.press('Enter')
    await expect(dialog.getByRole('alert')).toContainText('The end must come after the start.')

    // Up and down move a minute, with Shift an hour.
    await from.press('ArrowUp')
    await from.press('Shift+ArrowDown')
    const nudged = await from.inputValue()
    await to.fill('')
    await from.press('Enter')
    await expect(dialog).toHaveCount(0)
    const start = a.t + 60e3 - 3600e3
    expect(nudged).toMatch(/^\d{4}-\d\d-\d\d \d\d:\d\d$/)
    await expect(page).toHaveURL(new RegExp(`[#&?]t=${start}-now(&|$)`))
  })

  test('the calendar button opens the browser picker and a picked time lands in the field', async ({ page }) => {
    await open(page, 'logs')
    // Headless Chromium draws no picker window, so note the call and answer it the way the picker does.
    await page.evaluate(() => {
      ;(window as unknown as { picks: number }).picks = 0
      HTMLInputElement.prototype.showPicker = function () {
        ;(window as unknown as { picks: number }).picks++
      }
    })
    await page.keyboard.press('t')
    const dialog = page.getByRole('dialog', { name: 'Time range' })
    await dialog.getByRole('button', { name: 'Calendar for from' }).click()
    expect(await page.evaluate(() => (window as unknown as { picks: number }).picks)).toBe(1)
    const picked = await page.evaluate(() => {
      const d = new Date(Date.now() - 3 * 3600e3)
      const p = (n: number) => String(n).padStart(2, '0')
      return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(d.getHours())}:${p(d.getMinutes())}`
    })
    const ghost = dialog.locator('input[type="datetime-local"]').first()
    await ghost.evaluate((el: HTMLInputElement, v) => {
      el.value = v
      el.dispatchEvent(new Event('change', { bubbles: true }))
    }, picked)
    await expect(dialog.getByRole('textbox', { name: 'From', exact: true })).toHaveValue(picked.replace('T', ' '))
    await expect(dialog).toBeVisible()
  })
})

test('Ctrl K takes a time range', async ({ page }) => {
  await open(page, 'overview')
  await page.keyboard.press('Control+k')
  await page.getByPlaceholder(/^Search or run a command/).fill('last 2 hours')
  await page.keyboard.press('Enter')
  await expect(page).toHaveURL(/#logs\?.*t=2h(&|$)/)
})
