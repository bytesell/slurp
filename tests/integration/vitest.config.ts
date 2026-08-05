import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    // `runtime.test.ts` drives real DOM modules (params, cn, navigation), so it
    // needs a document. Without this the whole file failed on
    // "document is not defined" - 83 of 102 tests were red before any of them
    // asserted anything.
    environment: 'jsdom',
    // Runs once before any test file. It refuses to start when the WASM artifact these
    // tests import is older than `compiler/src`, because a stale build fails as a
    // plausible assertion mismatch rather than as a missing dependency.
    globalSetup: ['./setup.ts'],
  },
})
