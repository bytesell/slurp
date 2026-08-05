/**
 * A build entry that re-exports the reference data for the `llms-full.txt`
 * generator, plus the function that assembles that file.
 *
 * It exists so the generator can use the SAME data the server serves without
 * that data leaking into the package's public API surface (`src/index.ts`), and
 * without the generator needing a TypeScript loader to read `src/`.
 *
 * `renderLlmsFull` lives HERE rather than in the generator script so that the
 * test suite can assert the committed file matches without shelling out. If the
 * script owned the assembly, the test would have to reproduce the header and
 * would drift from it, which is the exact failure this whole arrangement exists
 * to prevent.
 */
import { TOPICS } from './reference/topics.js'

export { TOPICS, TOPIC_IDS, topicById } from './reference/topics.js'
export { ERROR_CODES } from './reference/errors.js'
export { FILTERS } from './reference/filters.js'
export { GOTCHAS } from './reference/gotchas.js'

const HEADER = `# Slurp, full reference

Generated from the @bytesell/slurp-mcp reference data, which is verified against the
compiler by rendering each documented construct and recording the result. Do not
edit this file by hand: run \`pnpm --filter @bytesell/slurp-mcp run build:llms\`.

Source: https://github.com/bytesell/slurp
`

/** The exact bytes `llms-full.txt` should contain. */
export function renderLlmsFull(): string {
  const body = TOPICS.map((t) => t.render()).join('\n\n---\n\n')
  return `${HEADER}\n---\n\n${body}\n`
}
