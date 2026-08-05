/**
 * The executable. Running this connects the server to stdio and blocks.
 *
 * STDOUT IS THE PROTOCOL. Anything written to stdout that is not a JSON-RPC
 * frame corrupts the stream, and the host disconnects with a parse error that
 * names neither the writer nor the line. So every message from this process
 * goes to stderr, and nothing in this package calls console.log.
 */
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js'
import { createServer, SERVER_NAME, SERVER_VERSION } from './server.js'

async function main(): Promise<void> {
  const server = createServer()
  const transport = new StdioServerTransport()
  await server.connect(transport)
  console.error(`${SERVER_NAME} ${SERVER_VERSION} listening on stdio`)
}

main().catch((error: unknown) => {
  console.error(`${SERVER_NAME}: fatal:`, error)
  process.exit(1)
})
