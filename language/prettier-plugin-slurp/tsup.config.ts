import { defineConfig } from 'tsup'

export default defineConfig({
  entry: ['src/index.ts'],
  format: ['esm'],
  dts: true,
  clean: true,
  // `prettier` stays external: it is a peer dependency and the host supplies it.
  external: ['prettier'],
  // `@slurp/language` is BUNDLED. tsup treats workspace dependencies as
  // external by default, which would publish a plugin whose only import of the
  // shared formatter cannot resolve for anyone outside this monorepo.
  noExternal: ['@slurp/language'],
})
