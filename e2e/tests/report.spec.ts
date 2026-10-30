import { expect, test } from '@playwright/test'
import { open } from './helpers'

test.use({ permissions: ['clipboard-read', 'clipboard-write'] })

test('a service panel copies a debug report as Markdown', async ({ page }) => {
  await open(page, 'stacks')
  await page.getByRole('row', { name: /demo_shop/ }).first().click()
  await page.getByRole('dialog').getByRole('button', { name: 'Debug report' }).click()
  await expect(page.locator('.toast')).toContainText('Debug report copied')
  const md = await page.evaluate(() => navigator.clipboard.readText())
  expect(md).toContain('# Debug report: demo_shop')
  expect(md).toContain('| Image | `alpine:3.20')
  expect(md).toContain('2 of 2 running')
  expect(md).toContain('## Most frequent errors and warnings')
  // Environment values can be secrets: only their number is in the report.
  expect(md).toContain('variables (values not included)')
  expect(md).not.toContain('logback')
})
