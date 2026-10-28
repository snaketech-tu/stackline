import { expect, test, type Page } from '@playwright/test'
import { clipped, open } from './helpers'

async function openErrors(page: Page, service: string) {
  await open(page, 'stacks')
  await page.getByRole('button', { name: new RegExp(`^Errors of .*${service}$`) }).click()
  const drawer = page.getByRole('dialog')
  await expect(drawer.getByRole('tab', { name: /Errors/ })).toHaveAttribute('aria-selected', 'true')
  await expect(drawer.locator('.ecard').first()).toBeVisible()
  return drawer
}

// shop logs Java traces; worker logs one very long message, the case that was cut off.
for (const [service, width] of [['shop', 1400], ['worker', 1400], ['worker', 1000]] as const) {
  test(`error groups of ${service} show their whole text at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 })
    const drawer = await openErrors(page, service)
    // Nothing in a card is cut off or hidden behind the edge of the drawer (0.3.0).
    expect(await clipped(page, '.drawer .ecard, .drawer .ecard *')).toEqual([])
    const edge = (await drawer.boundingBox())!
    for (const box of await drawer.locator('.ecard .ettl, .ecard pre.trace, .ecard .eacts').all()) {
      const b = (await box.boundingBox())!
      expect(b.x + b.width, `${await box.textContent()}`).toBeLessThanOrEqual(edge.x + edge.width + 1)
    }
    // Readable: the titles were once squeezed into tiny type.
    const size = await drawer.locator('.ecard .ettl').first().evaluate((el) => parseFloat(getComputedStyle(el).fontSize))
    expect(size).toBeGreaterThanOrEqual(12)
    if (service === 'shop') {
      // The Java trace arrives as one entry with all its lines.
      await expect(drawer.locator('.ecard pre.trace', { hasText: 'Checkout failed' }).first()).toContainText('at com.acme.shop.checkout.CheckoutService.total')
    } else {
      // The title is shortened with an ellipsis; the full message is right below it.
      const card = drawer.locator('.ecard', { hasText: 'import-catalog' })
      await expect(card.locator('.ettl')).toHaveText(/…$/)
      await expect(card.locator('pre.trace')).toContainText('next run scheduled by cron)')
    }
  })
}

test('an error group opens its lines in Logs', async ({ page }) => {
  const drawer = await openErrors(page, 'shop')
  await drawer.locator('.ecard').first().getByRole('button', { name: /^Show all .* in Logs$/ }).click()
  await expect(page.getByRole('dialog')).toHaveCount(0)
  await expect(page).toHaveURL(/#logs\?.*g=[0-9a-f]+/)
  await expect(page.locator('.stream .row').first()).toBeVisible()
})
