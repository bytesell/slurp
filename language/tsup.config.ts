import { defineConfig } from 'tsup'

export default defineConfig({
  entry: {
    index: 'src/index.ts',
    // `format` is exposed as its own entry so consumers that only need the
    // formatter (prettier-plugin-slurp) do not pull in `monaco.ts`, whose
    // `monaco-editor` import is an optional peer that is not installed here.
    format: 'src/format.ts',
  },
  format: ['esm'],
  dts: true,
  sourcemap: true,
  clean: true,
  // monaco-editor is an optional peer of `registerSlurp`; never bundle it.
  external: ['monaco-editor'],
})
