import { expect, test } from '@playwright/test'
import { open } from './helpers'

test('the demo stack is healthy although one of its containers exited on purpose', async ({ page }) => {
  await open(page)
  // `migrate` runs once and exits with 0: stopped on purpose, not failing.
  const row = page.locator('section.panel', { has: page.getByRole('heading', { name: 'Stacks' }) }).getByRole('row', { name: /demo/ })
  await expect(row).toContainText('healthy')
  await expect(page.locator('.kpi', { hasText: 'Errors · last hour' }).locator('.val')).not.toHaveText('0')
})

test('the connection menu is drawn over the host cards', async ({ page }) => {
  await open(page)
  await expect(page.locator('.kpi').first()).toBeVisible()
  await page.getByRole('button', { name: /This computer/ }).click()
  const menu = page.getByRole('menu', { name: 'Connections' })
  await expect(menu).toBeVisible()
  // Whatever is on top at the middle of each entry must be the menu itself.
  // A card painted over the menu (0.3.0) fails this even though the menu "is visible".
  const items = menu.getByRole('menuitemradio').or(menu.getByRole('menuitem'))
  await expect(items).toHaveCount(4)
  const covered = await items.evaluateAll((els) =>
    els
      .filter((el) => {
        const r = el.getBoundingClientRect()
        const top = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2)
        return !top || !el.closest('[role=menu]')!.contains(top)
      })
      .map((el) => el.textContent),
  )
  expect(covered).toEqual([])
  // The menu really overlaps the cards, otherwise the check above proves nothing.
  const m = (await menu.boundingBox())!
  const card = (await page.locator('.kpi').first().boundingBox())!
  expect(m.y + m.height).toBeGreaterThan(card.y)
  await page.keyboard.press('Escape')
  await expect(menu).toHaveCount(0)
})
