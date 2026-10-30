import { defineConfig, devices } from '@playwright/test'

export const TOKEN = process.env.STACKLINE_TOKEN ?? '0123456789abcdef0123456789abcdef'
const port = process.env.STACKLINE_PORT ?? '7171'

// The tests run against the demo stack in dev/demo (see README.md here).
// They share one Stackline and one store, so they run one after another.
export default defineConfig({
  testDir: 'tests',
  fullyParallel: false,
  workers: 1,
  retries: process.env.CI ? 1 : 0,
  timeout: 60_000,
  expect: { timeout: 15_000 },
  reporter: process.env.CI ? [['github'], ['html', { open: 'never' }]] : 'list',
  use: {
    baseURL: `http://127.0.0.1:${port}`,
    viewport: { width: 1400, height: 900 },
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  projects: [
    { name: 'ui', testIgnore: /read-only/, use: { ...devices['Desktop Chrome'], viewport: { width: 1400, height: 900 } } },
    // Last, so it sees every Docker call the other tests caused.
    { name: 'read-only', testMatch: /read-only/, dependencies: ['ui'] },
  ],
  webServer: {
    command: 'node serve.mjs',
    url: `http://127.0.0.1:${port}/`,
    reuseExistingServer: false,
    timeout: 30_000,
    stdout: 'pipe',
  },
})
