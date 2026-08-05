/**
 * Entry point for `@slurp/compiler-wasm`.
 *
 * This re-exports the wasm-bindgen glue in `pkg/` verbatim and adds one helper.
 * It exists for two reasons:
 *
 *   1. `pkg/` is a build artifact, so nothing outside this package should depend
 *      on its file names. Consumers import the package; the layout underneath
 *      can change.
 *   2. Each export is assigned individually rather than spread. `module.exports
 *      = { ...wasm }` is not statically analysable, so bundlers and Vite's CJS
 *      interop would fail to see the named exports and an ESM
 *      `import { parse } from '@slurp/compiler-wasm'` would come back undefined.
 *
 * The VS Code extension loads this with `require()` and reads `validate` off the
 * result, so the named exports must keep their wasm-bindgen names exactly.
 */
const wasm = require('./pkg/slurp_compiler.js')

module.exports.parse = wasm.parse
module.exports.render = wasm.render
module.exports.render_dev = wasm.render_dev
module.exports.validate = wasm.validate
module.exports.validate_full = wasm.validate_full
module.exports.extract_schema = wasm.extract_schema
module.exports.render_section = wasm.render_section

/**
 * `JSON.parse` for the JSON strings every export above returns.
 *
 * Every one of them serialises through `JsValue::from_str`, so the return value
 * is a string. Routing that through a named helper is what keeps the fact
 * visible at the call site.
 */
module.exports.parseJson = function parseJson(json) {
  return JSON.parse(json)
}
