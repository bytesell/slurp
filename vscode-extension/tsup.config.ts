import { defineConfig } from 'tsup'

export default defineConfig({
  entry: ['src/extension.ts'],
  format: ['cjs'],
  dts: true,
  clean: true,
  // `vscode` is supplied by the editor at run time and is never installable from npm.
  external: ['vscode'],
  // `@bytesell/slurp-language` is BUNDLED. tsup treats a workspace dependency as external by
  // default, and this one is `private: true`, so the packaged VSIX carried a `require` for a
  // package that does not exist on any registry. That fails at ACTIVATION, not at package time:
  // `vsce package` succeeds, the extension installs, and then does nothing with an error in the
  // host log. `language/prettier-plugin-slurp` hit the same trap and fixed it the same way.
  noExternal: ['@bytesell/slurp-language'],
})
