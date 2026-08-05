/**
 * Library entry point.
 *
 * Importing this must NEVER start a server. The executable is `src/bin.ts`,
 * which is a separate file for exactly that reason: an entry point that both
 * exports a factory and starts listening on stdio forces every consumer,
 * including the test suite, to guess whether it was invoked or imported. That
 * guess is a `process.argv[1]` comparison, which is subtly wrong on Windows
 * (drive letters, symlinked bin shims) and fails in a way that looks like the
 * server hanging.
 */
export { createServer, SERVER_NAME, SERVER_VERSION } from './server.js'
export type { Diagnostic } from './compiler.js'
export type { LintFinding, LintResult } from './tools/lint.js'
export type { SchemaResultPayload } from './tools/schema.js'
