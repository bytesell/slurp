import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    // `prettier-plugin-slurp` is a nested WORKSPACE PACKAGE inside this
    // directory and owns its own test run. Without this the default glob picks
    // its suite up here as well, so every plugin test would be reported twice
    // and a failure would be attributed to the wrong package.
    include: ['src/**/*.{test,spec}.ts'],
  },
})
