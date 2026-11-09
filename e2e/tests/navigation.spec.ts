import { expect, test } from '@playwright/test'
import { inspector, open, serviceRow } from './helpers'

test('the keyboard drills from a stack to its services and tasks, and Back returns', async ({ page }) => {
  await open(page, 'stacks')
  const bar = page.getByLabel('Actions')
  // Nothing is picked until the cursor moves, so no action can hit a row out of sight.
  await expect(bar.getByRole('button', { name: /^Services/ })).toBeDisabled()
  await page.keyboard.press('j')
  await expect(bar).toContainText('demo')
  await page.keyboard.press('Enter')
  await expect(page).toHaveURL(/#services\?stack=demo$/)
  await expect(page.locator('.scopechip')).toContainText('of stack demo')
  // Pick `shop` with the filter, then go down to its replicas.
  await page.keyboard.press('/')
  await page.keyboard.type('shop')
  await page.keyboard.press('Enter')
  await expect(bar).toContainText('demo_shop')
  await page.keyboard.press('Enter')
  await expect(page).toHaveURL(/#tasks\?service=demo%2Fshop$/)
  await expect(page.locator('.rlist tbody tr')).toHaveCount(2)
  await page.goBack()
  await expect(page).toHaveURL(/#services\?stack=demo$/)
  // The list is as it was left: filter and cursor.
  await expect(page.getByLabel('Filter services')).toHaveValue('shop')
  await expect(page.locator('.rlist tbody tr.cur')).toHaveAttribute('data-key', 'demo/shop')
  await page.getByLabel('Filter services').fill('')
})

test('a click inspects a service beside the list, and the inspector follows the cursor', async ({ page }) => {
  await open(page, 'services')
  await serviceRow(page, 'demo/shop').click()
  await expect(inspector(page, 'demo_shop')).toBeVisible()
  // The list stays usable next to it.
  await expect(serviceRow(page, 'demo/worker')).toBeVisible()
  await page.keyboard.press('j')
  await expect(inspector(page, 'demo_worker')).toBeVisible()
  await page.keyboard.press('Escape')
  await expect(page.locator('.drawer')).toHaveCount(0)
})

test('l follows a service in the log workspace while you browse', async ({ page }) => {
  await open(page, 'services')
  await serviceRow(page, 'demo/shop').click()
  await page.keyboard.press('Escape')
  await page.keyboard.press('l')
  const dock = page.getByRole('region', { name: 'Log workspace' })
  await expect(dock.getByRole('tab', { name: 'demo_shop' })).toHaveAttribute('aria-selected', 'true')
  await expect(dock.locator('.stream .row').first()).toBeVisible()
  // It stays open on other pages, and after a reload.
  await page.keyboard.press('g')
  await page.keyboard.press('o')
  await expect(page).toHaveURL(/#overview$/)
  await expect(dock.locator('.stream .row').first()).toBeVisible()
  await page.reload()
  await expect(dock.getByRole('tab', { name: 'demo_shop' })).toBeVisible()
  await dock.getByRole('button', { name: 'Close demo_shop' }).click()
  await expect(dock).toHaveCount(0)
})

test('the stack picker narrows every list, and : and ? open the palette and the keys', async ({ page }) => {
  await open(page, 'tasks')
  await page.locator('header.top').getByRole('button', { name: /^Stack/ }).click()
  await page.getByRole('menuitemradio', { name: /demo/ }).click()
  await expect(page.locator('.crumbs')).toContainText('demo')
  await page.locator('header.top').getByRole('button', { name: /^Stack/ }).click()
  await page.getByRole('menuitemradio', { name: /All stacks/ }).click()

  await page.keyboard.press(':')
  await expect(page.getByRole('dialog', { name: 'Jump to' })).toBeVisible()
  await page.keyboard.type('nodes')
  await page.keyboard.press('Enter')
  await expect(page).toHaveURL(/#nodes$/)
  await page.keyboard.press('?')
  await expect(page.getByRole('dialog', { name: 'Keys' })).toContainText('Follow its logs in the workspace below')
  await page.keyboard.press('Escape')
  await expect(page.getByRole('dialog', { name: 'Keys' })).toHaveCount(0)
})
