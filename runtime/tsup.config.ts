import { defineConfig } from 'tsup'

export default defineConfig({
  entry: {
    'slurp.core': 'src/core/index.ts',
    'slurp.fetch': 'src/fetch/index.ts',
    'slurp.infinite': 'src/infinite/index.ts',
  },
  format: ['esm'],
  splitting: false,
  minify: true,
  treeshake: true,
  dts: true,
  sourcemap: true,
  target: 'es2020',
})
