/**
 * The language reference, queryable.
 *
 * Two access patterns, because agents have two needs. Without a topic this
 * returns the INDEX plus the overview, which is the cheapest useful thing to
 * read. With a topic it returns that topic in full. A `query` narrows within a
 * topic, which matters for `filters` and `errors`, the two topics long enough
 * that returning all of them wastes context.
 *
 * The error-code lookup is deliberately reachable without knowing a topic name:
 * an agent holding a diagnostic wants to type the code, not to learn a taxonomy
 * first. `slurp_reference({ query: "UnsafeScriptInterpolation" })` works.
 */
import { TOPICS, topicById } from '../reference/topics.js'
import { ERROR_CODES } from '../reference/errors.js'
import { FILTERS } from '../reference/filters.js'
import { GOTCHAS } from '../reference/gotchas.js'

function index(): string {
  const rows = TOPICS.map((t) => `- \`${t.id}\` : ${t.summary}`).join('\n')
  return [
    '# Slurp reference',
    '',
    'Topics, for `slurp_reference({ topic })`:',
    '',
    rows,
    '',
    'A `query` narrows within a topic. An error code or filter name as the query works with no topic at all.',
  ].join('\n')
}

function renderErrorCode(code: string): string | null {
  const entry = ERROR_CODES.find((e) => e.code.toLowerCase() === code.toLowerCase())
  if (!entry) return null
  const lines = [`# ${entry.code}`, '']
  if (!entry.emitted) {
    lines.push(
      'THE COMPILER NEVER EMITS THIS CODE. It is declared for host integrations that add their own checks on top, so there is no compiler condition to go looking for.',
    )
    if (entry.note) lines.push('', entry.note)
    return lines.join('\n')
  }
  lines.push(`Severity: ${entry.severity}`, '', '## When it fires', '', entry.cause)
  if (entry.fix) lines.push('', '## Fix', '', entry.fix)
  if (entry.wrong) lines.push('', 'Wrong:', '```slurp', entry.wrong, '```')
  if (entry.right) lines.push('', 'Right:', '```slurp', entry.right, '```')
  if (entry.note) lines.push('', '## Note', '', entry.note)
  return lines.join('\n')
}

function renderFilter(name: string): string | null {
  const entry = FILTERS.find((f) => f.name.toLowerCase() === name.toLowerCase())
  if (!entry) return null
  const lines = [
    `# \`${entry.signature}\``,
    '',
    `A ${entry.kind} filter.${entry.kind === 'loop' ? ' Usable ONLY in an `{each}` header.' : ''}`,
    '',
    entry.summary,
    '',
    `## Null and wrong types`,
    '',
    entry.nullBehaviour,
  ]
  if (entry.trap) lines.push('', '## Trap', '', entry.trap)
  if (entry.example) lines.push('', '```slurp', entry.example, '```')
  return lines.join('\n')
}

/**
 * Narrow a rendered topic to the sections that mention the query.
 *
 * Markdown headings are the unit, so a match returns a coherent chunk rather
 * than an orphaned line. Falling back to the WHOLE topic on no match is
 * deliberate: an agent that guessed a term badly is better served by the topic
 * than by an empty result telling it nothing.
 */
function narrow(body: string, query: string): string {
  const needle = query.toLowerCase()
  const chunks = body.split(/\n(?=#{1,3} )/)
  const matched = chunks.filter((c) => c.toLowerCase().includes(needle))
  if (matched.length === 0 || matched.length === chunks.length) return body
  const header = chunks[0]?.startsWith('# ') ? chunks[0] : null
  const parts = header && !matched.includes(header) ? [header, ...matched] : matched
  return `${parts.join('\n\n')}\n\n(Narrowed to ${matched.length} of ${chunks.length} sections matching ${JSON.stringify(query)}. Omit \`query\` for the whole topic.)`
}

export function referenceLookup(topic?: string, query?: string): string {
  // An error code or filter name is answerable with no topic at all.
  if (!topic && query) {
    const asError = renderErrorCode(query)
    if (asError) return asError
    const asFilter = renderFilter(query)
    if (asFilter) return asFilter
  }

  if (!topic) {
    const overview = topicById('overview')
    const body = overview ? overview.render() : ''
    const head = query
      ? `No error code or filter named ${JSON.stringify(query)}. Pick a topic below and pass \`query\` again to search inside it.\n\n`
      : ''
    return `${head}${index()}\n\n---\n\n${body}`
  }

  const found = topicById(topic)
  if (!found) {
    return `Unknown topic ${JSON.stringify(topic)}.\n\n${index()}`
  }

  // Inside `errors` and `filters`, an exact name beats a text search.
  if (query) {
    if (found.id === 'errors') {
      const exact = renderErrorCode(query)
      if (exact) return exact
    }
    if (found.id === 'filters') {
      const exact = renderFilter(query)
      if (exact) return exact
    }
  }

  const body = found.render()
  return query ? narrow(body, query) : body
}

export { TOPICS, GOTCHAS }
