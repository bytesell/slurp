import { defineConfig } from 'tsup'

export default defineConfig({
  // Two entries on purpose: `index` is the importable library surface and must
  // not start anything, `bin` is the executable. See src/index.ts.
  entry: { index: 'src/index.ts', bin: 'src/bin.ts', 'llms-data': 'src/llms-data.ts' },
  format: ['esm'],
  splitting: false,
  // NOT minified, unlike the sibling packages. This artifact is executed by a
  // coding agent's MCP host, never shipped to a browser, so bytes do not matter
  // and a readable stack trace when a tool handler throws does.
  minify: false,
  treeshake: true,
  dts: true,
  sourcemap: true,
  target: 'node20',
  platform: 'node',
  // `@bytesell/slurp-compiler-wasm` is CommonJS and loads its .wasm with
  // `fs.readFileSync` relative to its own file. Bundling it would move that
  // read to a path where the .wasm does not exist, so it stays external and is
  // resolved from node_modules at run time.
  external: ['@bytesell/slurp-compiler-wasm', '@modelcontextprotocol/sdk'],
  // The shebang belongs on the executable only, but tsup's `banner` applies to
  // every entry. `onSuccess` prepends it to dist/bin.js alone, so importing the
  // library entry never carries a stray shebang line.
  onSuccess: async () => {
    const { readFileSync, writeFileSync, existsSync } = await import('node:fs')
    const binFile = new URL('dist/bin.js', import.meta.url)
    if (!existsSync(binFile)) return
    const current = readFileSync(binFile, 'utf8')
    if (current.startsWith('#!')) return
    writeFileSync(binFile, `#!/usr/bin/env node\n${current}`)
  },
})
