/**
 * The MCP server: five tools and three resources.
 *
 * `createServer` returns the configured server WITHOUT connecting a transport,
 * so the tests can drive the same object the stdio entry point drives. A server
 * whose only exercise is a hand-written unit test of its handlers is not
 * actually known to work over the wire, so `test/smoke.test.ts` connects a real
 * client to this over a real stdio pipe and completes real calls.
 *
 * WHY FIVE TOOLS. The set is deliberately small: an agent choosing among five
 * well-separated tools picks correctly far more often than one choosing among a
 * dozen overlapping ones. The separation is by QUESTION, not by compiler entry
 * point:
 *
 *   slurp_validate  - is this correct?          (compiler diagnostics)
 *   slurp_render    - what does it produce?     (HTML plus dev-mode warnings)
 *   slurp_lint      - is it silently wrong?     (what the compiler will not say)
 *   slurp_schema    - what is editable here?    (the declared surface)
 *   slurp_reference - how do I write this?      (the language)
 *
 * `slurp_lint` earns its place because the compiler is genuinely silent about
 * its findings; it is not a second opinion on the same diagnostics. A separate
 * `slurp_explain` was considered and REJECTED: it would have been a lookup into
 * the same table `slurp_reference` already serves, and an error code passed as
 * `query` with no topic reaches that table directly.
 */
import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js'
import { z } from 'zod'

import { renderTemplate, validateSource } from './compiler.js'
import type { Diagnostic } from './compiler.js'
import { lintTemplate } from './tools/lint.js'
import { templateSchema } from './tools/schema.js'
import { referenceLookup } from './tools/reference.js'
import { TOPICS, topicById, TOPIC_IDS } from './reference/topics.js'

export const SERVER_NAME = '@bytesell/slurp-mcp'
export const SERVER_VERSION = '0.1.0'

/** The default file name when a caller passes source with no path of its own. */
const DEFAULT_FILE = 'template.slurp'

function textResult(text: string, isError = false) {
  return { content: [{ type: 'text' as const, text }], isError }
}

function jsonResult(value: unknown, isError = false) {
  return textResult(JSON.stringify(value, null, 2), isError)
}

function countBySeverity(diagnostics: Diagnostic[]): { errors: number; warnings: number } {
  let errors = 0
  let warnings = 0
  for (const d of diagnostics) {
    if (d.severity === 'error') errors += 1
    else warnings += 1
  }
  return { errors, warnings }
}

const sourceShape = {
  source: z.string().describe('The .slurp template source to check.'),
  file: z
    .string()
    .optional()
    .describe(
      `Path to report in diagnostics. Cosmetic: nothing is read from disk. Defaults to "${DEFAULT_FILE}".`,
    ),
}

const middlewareShape = {
  is_middleware: z
    .boolean()
    .optional()
    .describe(
      'Set true ONLY for a file that will be compiled as middleware. The rules genuinely differ: request.* and {redirect} are legal only in middleware, and most other path roots are illegal there. Middleware is a compile option, not a filename convention, so this defaults to false and matches what the slurp CLI does.',
    ),
}

export function createServer(): McpServer {
  const server = new McpServer(
    { name: SERVER_NAME, version: SERVER_VERSION },
    {
      capabilities: { tools: {}, resources: {} },
      instructions: [
        'Tools for writing .slurp templates correctly.',
        '',
        'Run slurp_validate after every edit: it is the whole correction loop, and it reports exactly what `slurp build` enforces, including the compile-time security walk.',
        'Run slurp_lint as well. The two do NOT overlap: lint reports the constructs that compile cleanly and then render wrong with no diagnostic at all, which the compiler will never tell you about.',
        'Read slurp_reference({ topic: "gotchas" }) before writing your first template. The single most common mistake is that a quoted COMPONENT prop is a literal string while a quoted ELEMENT attribute interpolates.',
      ].join('\n'),
    },
  )

  registerTools(server)
  registerResources(server)
  return server
}

function registerTools(server: McpServer): void {
  server.registerTool(
    'slurp_validate',
    {
      title: 'Validate a Slurp template',
      description: [
        'Compile-check a .slurp template and return structured diagnostics: code, message, file, line, column, severity.',
        '',
        'This reports exactly what `slurp build` enforces, which is more than parsing. It includes the compile-time security walk, so an unfiltered ${ } in a <script> body, a `| js` slot in JavaScript statement position, env.SLURP_SECRET_* access, request.* outside middleware and {redirect} outside middleware are all reported here.',
        '',
        'Run this after every edit. It is the fastest way to close the loop.',
        '',
        'Two limits worth knowing. The security walk stops at its FIRST violation, so at most one security diagnostic appears per call; fix it and run again to see the next. And nothing here resolves imports across files, so a missing component or layout is not reported (it renders as a placeholder at run time).',
      ].join('\n'),
      inputSchema: {
        ...sourceShape,
        ...middlewareShape,
        files: z
          .record(z.string())
          .optional()
          .describe(
            'Optionally validate several templates in one call, as a map of path to source. Each is validated INDEPENDENTLY; imports are still not resolved between them.',
          ),
      },
    },
    async (args) => {
      const isMiddleware = args.is_middleware ?? false
      const entries: Array<[string, string]> = args.files
        ? Object.entries(args.files)
        : [[args.file ?? DEFAULT_FILE, args.source]]

      const diagnostics: Diagnostic[] = []
      for (const [path, src] of entries) {
        diagnostics.push(...validateSource(src, path, isMiddleware))
      }

      const counts = countBySeverity(diagnostics)
      return jsonResult({
        ok: counts.errors === 0,
        files_checked: entries.length,
        ...counts,
        diagnostics,
        ...(counts.errors === 0 && counts.warnings === 0
          ? {
              note: 'No diagnostics. Note that a clean validate does NOT mean the template renders correctly: run slurp_lint for the silent-failure classes the compiler does not report.',
            }
          : {}),
      })
    },
  )

  server.registerTool(
    'slurp_render',
    {
      title: 'Render a Slurp template',
      description: [
        'Render a template against a JSON context and return the HTML plus every diagnostic from both the validation and the render pass.',
        '',
        'Rendering uses DEVELOPMENT mode on purpose. Two advisories exist only in that mode and are dropped in production: CSS-structural characters stripped from a style value slot, and an undeclared slot in a JavaScript-evaluated attribute. {debug expr} also renders only here. Loop truncation is NOT one of them - it warns in either mode, though `slurp build` prints warnings only with -v.',
        '',
        'Rendering is deterministic and side-effect free: no network request is made, no file is read, and {fetch} picks a branch by reading its name out of the context you pass rather than fetching anything.',
        '',
        'Components and layouts CANNOT be resolved here, because there is no cross-file registry. Each one renders as a <div data-slurp-component="Name" data-slurp-props="..."> placeholder, which is the same thing the real compiler does for an unresolved import.',
      ].join('\n'),
      inputSchema: {
        ...sourceShape,
        ...middlewareShape,
        context: z
          .record(z.unknown())
          .optional()
          .describe(
            'The render context, as a JSON object. Every top-level key becomes a global the template can read. Defaults to {}.',
          ),
      },
    },
    async (args) => {
      const file = args.file ?? DEFAULT_FILE
      const outcome = renderTemplate(
        args.source,
        file,
        args.context ?? {},
        args.is_middleware ?? false,
      )
      const counts = countBySeverity(outcome.diagnostics)
      const placeholders = (outcome.html.match(/data-slurp-component="/g) ?? []).length

      return jsonResult({
        ok: outcome.ok,
        html: outcome.html,
        ...counts,
        diagnostics: outcome.diagnostics,
        ...(placeholders > 0
          ? {
              note: `${placeholders} component placeholder(s) in the output. Components cannot be resolved without a cross-file registry, so this is expected here and is not evidence of a broken import.`,
            }
          : {}),
      })
    },
  )

  server.registerTool(
    'slurp_lint',
    {
      title: 'Find silent failures in a Slurp template',
      description: [
        'Report constructs that compile cleanly and then render WRONG with no diagnostic anywhere.',
        '',
        'This does not overlap slurp_validate. Every rule here describes something both `slurp validate` and `slurp build` accept with zero output, and that then renders the empty string or the wrong string. An agent cannot discover any of them from a failing build, because the build does not fail.',
        '',
        'The rules: a quoted component prop carrying interpolation syntax (it is a literal string, unlike an element attribute); a filter argument that is not a literal; a function call (there are no callable functions, every call is null); a `{$let}` name read back as a template value; `.length` on an array; an unquoted number in a component prop position; and a nested double quote inside ${ } that ends its attribute early.',
        '',
        'Each finding carries a concrete fix.',
      ].join('\n'),
      inputSchema: sourceShape,
    },
    async (args) => {
      const file = args.file ?? DEFAULT_FILE
      const result = lintTemplate(args.source, file)
      return jsonResult({
        clean: result.clean,
        findings: result.findings,
        ...(result.diagnostics.length > 0
          ? {
              parse_diagnostics: result.diagnostics,
              note: 'This template does not parse cleanly, so linting may be incomplete. Run slurp_validate first.',
            }
          : {}),
        ...(result.clean && result.diagnostics.length === 0
          ? { note: 'No silent-failure patterns found.' }
          : {}),
      })
    },
  )

  server.registerTool(
    'slurp_schema',
    {
      title: 'Read the editable surface of a Slurp template',
      description: [
        'Return what a template declares in frontmatter: its section { } or block { } schema with every setting, kind, default and nested block, plus its props, its imports and its middleware name.',
        '',
        'This is how you discover what is editable before changing a section, and what a component expects before calling it.',
        '',
        'A schema of null is NOT an error: most templates declare none. A block file declares block { } instead of section { }, and a block TYPE is its FILE NAME rather than the name it declares.',
      ].join('\n'),
      inputSchema: sourceShape,
    },
    async (args) => {
      const file = args.file ?? DEFAULT_FILE
      const result = templateSchema(args.source, file)
      const declaresNothing =
        !result.section && !result.block && result.props.length === 0 && result.imports.length === 0
      return jsonResult({
        ...result,
        ...(declaresNothing
          ? {
              note: 'This template declares no schema, props or imports. That is normal for an ordinary page and is not an error.',
            }
          : {}),
      })
    },
  )

  server.registerTool(
    'slurp_reference',
    {
      title: 'Look up the Slurp language reference',
      description: [
        'The language reference, derived from the compiler and checked against it by rendering.',
        '',
        `Topics: ${TOPIC_IDS.join(', ')}.`,
        '',
        'Call with no arguments for the index plus the overview. Pass an error code or a filter name as `query` with NO topic to jump straight to it, which is what you want when holding a diagnostic from slurp_validate. Pass both to search inside a topic.',
      ].join('\n'),
      inputSchema: {
        topic: z
          .enum(TOPIC_IDS as [string, ...string[]])
          .optional()
          .describe('Which topic to return. Omit for the index plus the overview.'),
        query: z
          .string()
          .optional()
          .describe(
            'An error code, a filter name, or free text. With no topic, an exact error-code or filter-name match is answered directly.',
          ),
      },
    },
    async (args) => textResult(referenceLookup(args.topic, args.query)),
  )
}

/**
 * Resources, so an agent can pull the whole reference into context without a
 * tool round-trip per topic. These are the three an agent actually wants up
 * front; the per-topic detail stays behind `slurp_reference` rather than being
 * duplicated as fifteen resources nobody lists.
 */
function registerResources(server: McpServer): void {
  server.registerResource(
    'slurp-language-reference',
    'slurp://reference/language',
    {
      title: 'Slurp language reference',
      description:
        'The complete language reference: tags, expressions, filters, components, script rules, frontmatter, schemas and limits.',
      mimeType: 'text/markdown',
    },
    async (uri) => ({
      contents: [
        {
          uri: uri.href,
          mimeType: 'text/markdown',
          text: TOPICS.map((t) => t.render()).join('\n\n---\n\n'),
        },
      ],
    }),
  )

  server.registerResource(
    'slurp-error-codes',
    'slurp://reference/errors',
    {
      title: 'Slurp error codes',
      description:
        'Every ErrorCode the compiler declares, which ones it actually emits, and a worked fix for each.',
      mimeType: 'text/markdown',
    },
    async (uri) => ({
      contents: [
        {
          uri: uri.href,
          mimeType: 'text/markdown',
          text: topicById('errors')?.render() ?? '',
        },
      ],
    }),
  )

  server.registerResource(
    'slurp-gotchas',
    'slurp://reference/gotchas',
    {
      title: 'Slurp gotchas',
      description:
        'The silent failures, each with the fix and with whether a tool will catch it for you.',
      mimeType: 'text/markdown',
    },
    async (uri) => ({
      contents: [
        {
          uri: uri.href,
          mimeType: 'text/markdown',
          text: topicById('gotchas')?.render() ?? '',
        },
      ],
    }),
  )
}
