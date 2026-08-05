/**
 * End-to-end over a real stdio transport.
 *
 * Separate from the unit tests by design. Those call the tool logic directly,
 * which proves the logic and proves nothing about the server:
 * a schema the SDK rejects, a handler registered under a name that does not
 * match its description, a result shape the protocol will not carry, or a stray
 * write to stdout would all leave those tests green and the server unusable in
 * the only way it is ever used. So this connects an actual MCP client to an
 * actual child process over an actual pipe and completes real calls.
 *
 * It runs against `dist/`, not `src/`, because `dist/` is what a host executes.
 * A build is therefore a prerequisite; the test says so plainly rather than
 * failing with a module-not-found.
 */
import { existsSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, resolve } from 'node:path'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'

import { Client } from '@modelcontextprotocol/sdk/client/index.js'
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js'

const here = dirname(fileURLToPath(import.meta.url))
const binPath = resolve(here, '..', 'dist', 'bin.js')

let client: Client
let transport: StdioClientTransport

/** Tool results arrive as content blocks; this is the text of the first one. */
function firstText(result: unknown): string {
  const content = (result as { content?: Array<{ type: string; text?: string }> }).content
  expect(Array.isArray(content)).toBe(true)
  const block = content?.[0]
  expect(block?.type).toBe('text')
  return block?.text ?? ''
}

function firstJson(result: unknown): Record<string, unknown> {
  return JSON.parse(firstText(result)) as Record<string, unknown>
}

/**
 * The text of a resource's first content block.
 *
 * A resource content block is a union of a text form and a blob form, so the
 * narrowing is real rather than a cast for the type checker's benefit: a server
 * that started returning binary here would fail this assertion instead of
 * silently reading `undefined`.
 */
function resourceText(result: {
  contents: Array<{ uri: string; text?: string; blob?: string; mimeType?: string }>
}): string {
  const block = result.contents[0]
  expect(block, 'resource returned no content').toBeDefined()
  expect(typeof block!.text, 'resource content was not text').toBe('string')
  return block!.text as string
}

beforeAll(async () => {
  if (!existsSync(binPath)) {
    throw new Error(
      `dist/bin.js is missing. The smoke test runs the built server, so run \`pnpm build\` in mcp/ first. Looked for: ${binPath}`,
    )
  }
  transport = new StdioClientTransport({ command: process.execPath, args: [binPath] })
  client = new Client({ name: 'slurp-mcp-smoke-test', version: '0.0.0' })
  await client.connect(transport)
}, 60_000)

afterAll(async () => {
  await client?.close()
})

describe('stdio server', () => {
  it('completes the handshake and reports its identity', () => {
    const info = client.getServerVersion()
    expect(info?.name).toBe('@bytesell/slurp-mcp')
  })

  it('advertises exactly the five tools, each with a description', async () => {
    const { tools } = await client.listTools()
    const names = tools.map((t) => t.name).sort()
    expect(names).toEqual([
      'slurp_lint',
      'slurp_reference',
      'slurp_render',
      'slurp_schema',
      'slurp_validate',
    ])
    for (const tool of tools) {
      expect(tool.description, tool.name).toBeTruthy()
      expect(tool.inputSchema, tool.name).toBeTruthy()
    }
  })

  it('advertises the three resources', async () => {
    const { resources } = await client.listResources()
    const uris = resources.map((r) => r.uri).sort()
    expect(uris).toEqual([
      'slurp://reference/errors',
      'slurp://reference/gotchas',
      'slurp://reference/language',
    ])
  })

  it('reads the language reference resource', async () => {
    const result = await client.readResource({ uri: 'slurp://reference/language' })
    const text = resourceText(result)
    expect(text.length).toBeGreaterThan(5000)
    expect(text).toContain('# Slurp')
    expect(result.contents[0]?.mimeType).toBe('text/markdown')
  })

  it('reads the error-codes resource', async () => {
    const result = await client.readResource({ uri: 'slurp://reference/errors' })
    expect(resourceText(result)).toContain('UnsafeScriptInterpolation')
  })

  it('reads the gotchas resource', async () => {
    const result = await client.readResource({ uri: 'slurp://reference/gotchas' })
    expect(resourceText(result)).toContain('quoted component prop')
  })

  // The real round trip: a broken template in, a structured diagnostic out.
  it('slurp_validate returns a structured diagnostic over the wire', async () => {
    const result = await client.callTool({
      name: 'slurp_validate',
      arguments: { source: '<script>var x = ${ name };</script>', file: 'hero.slurp' },
    })
    const payload = firstJson(result)
    expect(payload['ok']).toBe(false)
    const diagnostics = payload['diagnostics'] as Array<Record<string, unknown>>
    expect(diagnostics[0]!['code']).toBe('UnsafeScriptInterpolation')
    expect(diagnostics[0]!['file']).toBe('hero.slurp')
    expect(diagnostics[0]!['severity']).toBe('error')
    expect(typeof diagnostics[0]!['line']).toBe('number')
  })

  it('slurp_validate accepts a multi-file map', async () => {
    const result = await client.callTool({
      name: 'slurp_validate',
      arguments: {
        source: '',
        files: {
          'a.slurp': '<p>${ ok }</p>',
          'b.slurp': '{each x in y}<li></li>',
        },
      },
    })
    const payload = firstJson(result)
    expect(payload['files_checked']).toBe(2)
    const diagnostics = payload['diagnostics'] as Array<Record<string, unknown>>
    expect(diagnostics.every((d) => d['file'] === 'b.slurp')).toBe(true)
  })

  it('slurp_render returns HTML and dev-mode diagnostics', async () => {
    const result = await client.callTool({
      name: 'slurp_render',
      arguments: {
        source: '{each p in products}<li>${ p.name | upper }</li>{/each}',
        context: { products: [{ name: 'widget' }, { name: 'gizmo' }] },
      },
    })
    const payload = firstJson(result)
    expect(payload['ok']).toBe(true)
    expect(payload['html']).toBe('<li>WIDGET</li><li>GIZMO</li>')
  })

  it('slurp_lint reports the literal-prop gotcha with a fix', async () => {
    const result = await client.callTool({
      name: 'slurp_lint',
      arguments: { source: '<Card title="Hi ${name}" />' },
    })
    const payload = firstJson(result)
    expect(payload['clean']).toBe(false)
    const findings = payload['findings'] as Array<Record<string, unknown>>
    expect(findings[0]!['rule']).toBe('component-prop-literal-interpolation')
    expect(String(findings[0]!['fix'])).toContain('template literal')
  })

  it('slurp_schema returns the declared editing surface', async () => {
    const result = await client.callTool({
      name: 'slurp_schema',
      arguments: {
        source: [
          '---',
          'section {',
          '  name: "Hero"',
          '  settings { heading: text = "Welcome" }',
          '}',
          '---',
          '<h1>${ section.settings.heading }</h1>',
        ].join('\n'),
        file: 'sections/hero.slurp',
      },
    })
    const payload = firstJson(result)
    const section = payload['section'] as Record<string, unknown>
    expect(section['name']).toBe('Hero')
    expect((section['settings'] as Array<Record<string, unknown>>)[0]!['default']).toBe('Welcome')
  })

  it('slurp_reference answers an error code with no topic', async () => {
    const result = await client.callTool({
      name: 'slurp_reference',
      arguments: { query: 'JsTemplateLiteralInAttribute' },
    })
    const text = firstText(result)
    expect(text).toContain('# JsTemplateLiteralInAttribute')
    expect(text).toContain('## Fix')
  })

  it('slurp_reference returns a named topic', async () => {
    const result = await client.callTool({
      name: 'slurp_reference',
      arguments: { topic: 'gotchas' },
    })
    expect(firstText(result)).toContain('# Gotchas')
  })

  // A validate-fix-validate cycle, which is the loop the whole server exists to
  // close. If this passes, an agent can genuinely correct itself.
  it('closes the correction loop: broken, then diagnosed, then fixed, then clean', async () => {
    const broken = '<button @click="add(${ id | js })">Add</button>'
    const first = firstJson(
      await client.callTool({ name: 'slurp_validate', arguments: { source: broken } }),
    )
    expect(first['ok']).toBe(false)
    const code = (first['diagnostics'] as Array<Record<string, unknown>>)[0]!['code']

    const help = firstText(
      await client.callTool({ name: 'slurp_reference', arguments: { query: String(code) } }),
    )
    expect(help).toContain('json')

    const fixed = '<button @click="add(${ id | json })">Add</button>'
    const second = firstJson(
      await client.callTool({ name: 'slurp_validate', arguments: { source: fixed } }),
    )
    expect(second['ok']).toBe(true)
    expect(second['diagnostics']).toEqual([])
  })
})
