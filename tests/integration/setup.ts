/**
 * Refuses to run this suite against a WASM build older than the compiler source.
 *
 * These tests import `@bytesell/slurp-compiler-wasm`, which is a BUILD ARTIFACT produced by
 * `wasm-pack` and deliberately not committed. Nothing in `pnpm test` rebuilds it, so
 * after any change under `compiler/src` the suite silently exercises the PREVIOUS
 * compiler. That is worse than a missing file, because the failure it produces is a
 * real-looking assertion mismatch pointing at correct code: it cost a debugging round
 * when a renderer change made two tests fail against a WASM built minutes earlier.
 *
 * A presence check cannot catch this - the file exists, it is just old - so this
 * compares modification times and fails with the command to run. It is deliberately
 * a hard failure rather than a warning, because the whole value is that nobody has to
 * notice.
 *
 * The comparison is one-directional on purpose: a wasm NEWER than every source file is
 * fine, and clock skew or a checkout that rewrites mtimes can only ever produce a
 * spurious REBUILD, never a spurious pass.
 */
import { existsSync, readdirSync, statSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const repoRoot = resolve(here, '..', '..')
const wasmPath = join(repoRoot, 'compiler-wasm', 'pkg', 'slurp_compiler_bg.wasm')
const compilerSrc = join(repoRoot, 'compiler', 'src')

const REBUILD = 'pnpm --filter @bytesell/slurp-compiler-wasm build'

function newestMtimeMs(dir: string): number {
  let newest = 0
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name)
    const mtime = entry.isDirectory() ? newestMtimeMs(full) : statSync(full).mtimeMs
    if (mtime > newest) newest = mtime
  }
  return newest
}

// Vitest requires a globalSetup module to export `setup`, `teardown` or a default. Throwing at
// import time instead reports "invalid globalSetup file" and hides the real message.
export function setup(): void {
  if (!existsSync(wasmPath)) {
    throw new Error(
      `The WASM build is missing, so these tests have nothing to run against.\n\nRun: ${REBUILD}\n\nLooked for: ${wasmPath}`,
    )
  }

  const wasmMtime = statSync(wasmPath).mtimeMs
  const srcMtime = newestMtimeMs(compilerSrc)

  if (srcMtime > wasmMtime) {
    const age = Math.round((srcMtime - wasmMtime) / 1000)
    throw new Error(
      `The WASM build is ${age}s older than compiler/src, so this suite would test the PREVIOUS compiler and report failures against code that is correct.\n\nRun: ${REBUILD}`,
    )
  }
}
