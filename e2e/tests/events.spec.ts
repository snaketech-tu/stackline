import { expect, test } from '@playwright/test'
import { open } from './helpers'

// `migrate` prints one line and exits with 0 right after the demo starts.
test('a service panel lists when its containers stopped, and opens the lines before', async ({ page }) => {
  await open(page, 'stacks')
  await page.getByRole('row', { name: /demo_migrate/ }).click()
  const drawer = page.getByRole('dialog')
  const list = drawer.locator('div', { has: page.getByRole('heading', { name: /Stops and crashes/ }) })
  const row = list.getByRole('row', { name: /exited · exit 0/ })
  await expect(row).toBeVisible()
  await expect(row).toContainText('#1')
  // An exit on purpose is not a crash.
  await expect(drawer.getByRole('heading', { name: /Stops and crashes/ })).not.toContainText(/\d+ crash/)

  await row.getByRole('button', { name: 'Lines before' }).click()
  await expect(page.locator('.chiprow')).toContainText('demo_migrate')
  await expect(page.locator('.chiprow')).toContainText('replica:1')
  await expect(page.locator('.stream')).toContainText('Applied 3 migrations')
  await expect(page.locator('.legend')).toContainText('Starts and stops')
})
