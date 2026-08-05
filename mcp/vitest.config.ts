import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    environment: 'node',
    include: ['test/**/*.test.ts'],
    // The smoke test spawns a child process and completes a full MCP handshake,
    // which is slower than the default per-test budget allows on a cold start.
    testTimeout: 30_000,
    hookTimeout: 60_000,
  },
})
