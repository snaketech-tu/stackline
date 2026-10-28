import { expect, test } from '@playwright/test'
import { nav, open } from './helpers'

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
  await open(page, 'stacks')
  await page.getByRole('button', { name: /^Logs of .*worker$/ }).click()
  await expect(page).toHaveURL(/#logs$/)
  const rows = page.locator('.stream .row .svc')
  await expect(rows.first()).toContainText('worker')
  await page.reload()
  await expect(rows.first()).toContainText('worker')
  expect(await rows.allTextContents()).not.toContainEqual(expect.stringMatching(/shop|app|caddy/))
})

test('the Stacks filters stay when you go to another page and back', async ({ page }) => {
  await open(page, 'stacks')
  await page.getByPlaceholder('Filter by service, image or stack').fill('shop')
  await expect(page.getByRole('row', { name: /worker/ })).toHaveCount(0)
  await nav(page, 'Overview')
  await nav(page, 'Stacks & services')
  await expect(page.getByPlaceholder('Filter by service, image or stack')).toHaveValue('shop')
  await expect(page.getByRole('row', { name: /worker/ })).toHaveCount(0)
  await page.reload()
  await expect(page.getByPlaceholder('Filter by service, image or stack')).toHaveValue('shop')
  await page.getByPlaceholder('Filter by service, image or stack').fill('')
})
