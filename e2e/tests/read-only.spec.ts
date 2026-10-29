import { expect, test } from '@playwright/test'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
const RUN = join(import.meta.dirname, '..', '.run')

// Every call Stackline made to Docker during the other tests went through the
// guard started by serve.mjs. Stackline must only ever read.
test('Stackline only sent reads to Docker', () => {
  const calls = readFileSync(join(RUN, 'docker-calls.log'), 'utf8').split('\n').filter(Boolean)
  expect(calls.length).toBeGreaterThan(20)
  expect(calls.some((c) => /\/containers\/[^/]+\/logs/.test(c))).toBe(true)
  expect(calls.filter((c) => !/^(GET|HEAD) /.test(c))).toEqual([])
})
