import { expect, test, type Page } from '@playwright/test'
import { open } from './helpers'

const storeLine = (page: Page) => page.locator('section.panel', { has: page.getByRole('heading', { name: 'Local store' }) }).locator('.set .l').first()
const entries = async (page: Page) => Number(((await storeLine(page).textContent()) ?? '').match(/([\d,.]+) entries/)?.[1].replace(/[,.]/g, '') ?? -1)

test('the store size in Preferences follows new log lines', async ({ page }) => {
  await open(page, 'prefs')
  await expect.poll(() => entries(page)).toBeGreaterThan(0)
  const first = await entries(page)
  // The demo stack logs every second; the page refreshes the numbers every 15 s (0.3.1 never did).
  await expect.poll(() => entries(page), { timeout: 40_000 }).toBeGreaterThan(first)
})

test('a changed setting is saved and survives a reload', async ({ page }) => {
  await open(page, 'prefs')
  const keep = page.getByLabel('Keep logs for')
  await expect(keep).toHaveValue('2')
  await keep.selectOption('14')
  await expect(page.locator('.saved.on')).toBeVisible()
  await page.reload()
  await expect(page.getByLabel('Keep logs for')).toHaveValue('14')
  await page.getByLabel('Keep logs for').selectOption('2')
  await expect(page.locator('.saved.on')).toBeVisible()
})

test('after clearing the store, logs are collected again', async ({ page }) => {
  await open(page, 'prefs')
  await expect.poll(() => entries(page)).toBeGreaterThan(0)
  await page.getByRole('button', { name: 'Clear stored logs…' }).click()
  await page.getByRole('button', { name: 'Delete', exact: true }).click()
  await expect(page.getByRole('status')).toContainText('Stored logs cleared')
  // Collectors start over instead of waiting for lines newer than before the clear (0.3.1).
  await expect.poll(() => entries(page), { timeout: 40_000 }).toBeGreaterThan(100)
  await open(page, 'logs')
  await expect(page.locator('.stream .row').first()).toBeVisible()
})
