import { expect, type Page } from '@playwright/test'
import { TOKEN } from '../playwright.config'

/** Opens Stackline signed in, on the page `view`, once the demo stack shows. */
export async function open(page: Page, view = 'overview') {
  await page.goto(`/?token=${TOKEN}#${view}`)
  if (view !== 'prefs' && view !== 'connections') await expect(page.locator('main.view .center-empty')).toHaveCount(0)
}

/** Goes to another page with the sidebar, the way people do. */
export async function nav(page: Page, label: string | RegExp) {
  await page.locator('nav.nav').getByRole('link', { name: label }).click()
}

/** Elements of `selector` whose content is wider or taller than their box shows. */
export async function clipped(page: Page, selector: string) {
  return page.locator(selector).evaluateAll((els) =>
    els
      .filter((el) => {
        const s = getComputedStyle(el)
        const hides = (v: string) => v === 'hidden' || v === 'clip'
        const wide = el.scrollWidth > el.clientWidth + 1 && (hides(s.overflowX) || s.textOverflow === 'ellipsis')
        const tall = el.scrollHeight > el.clientHeight + 1 && hides(s.overflowY)
        return wide || tall
      })
      .map((el) => (el.textContent ?? '').slice(0, 80)),
  )
}
