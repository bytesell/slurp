#!/usr/bin/env node
/**
 * Writes `llms-full.txt` at the repository root from the SAME reference data the
 * MCP server serves.
 *
 * The point is that the two cannot drift. A hand-maintained `llms-full.txt`
 * beside a tool-served reference is two copies of the same claims, and the copy
 * nobody runs is the one that goes stale. Generating it means the reference
 * tests, which re-derive their claims from the real compiler, cover this file
 * too.
 *
 * `pnpm build:llms` regenerates it. `pnpm check:llms` verifies the committed
 * file matches without writing, which is what CI wants.
 *
 * The assembly itself lives in `src/llms-data.ts`, not here, so the test suite
 * can check the committed file without shelling out and without reproducing
 * this file's header.
 */
import { readFileSync, writeFileSync, existsSync } from 'node:fs'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { dirname, resolve } from 'node:path'

const here = dirname(fileURLToPath(import.meta.url))
const distEntry = resolve(here, '..', 'dist', 'llms-data.js')
const outFile = resolve(here, '..', '..', 'llms-full.txt')
const check = process.argv.includes('--check')

if (!existsSync(distEntry)) {
  console.error(
    `[llms] dist/llms-data.js is missing. Run \`pnpm build\` in mcp/ first. Looked for: ${distEntry}`,
  )
  process.exit(1)
}

// `pathToFileURL` is required, not decoration: on Windows a bare absolute path
// starts with a drive letter, which Node's ESM loader reads as an unsupported
// URL scheme and rejects with ERR_UNSUPPORTED_ESM_URL_SCHEME.
const { renderLlmsFull, TOPICS } = await import(pathToFileURL(distEntry).href)
const content = renderLlmsFull()

if (check) {
  const existing = existsSync(outFile) ? readFileSync(outFile, 'utf8') : ''
  if (existing !== content) {
    console.error(
      '[llms] llms-full.txt is out of date. Run `pnpm --filter @slurp/mcp run build:llms`.',
    )
    process.exit(1)
  }
  console.log('[llms] llms-full.txt is up to date.')
  process.exit(0)
}

writeFileSync(outFile, content)
console.log(`[llms] wrote ${outFile} (${content.length} bytes, ${TOPICS.length} topics)`)
