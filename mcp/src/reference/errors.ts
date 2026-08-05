/**
 * Every `ErrorCode` the compiler declares, and for the ones it actually emits, a
 * worked fix.
 *
 * WHY THE `emitted` FLAG IS THE IMPORTANT FIELD. The enum declares 21 variants
 * and the compiler emits 13 of them. The other 8 exist for host integrations
 * that layer their own checks on top (a dev server, a publish gate), and
 * documenting them as compiler errors would send a reader hunting for a
 * condition the compiler will never report. Two of the eight are subtler than
 * "unused" and say so in their notes: the compiler DOES detect both conditions,
 * and reports them under a different code or handles them silently.
 *
 * VERIFICATION. The list is the enum in `compiler/src/error.rs`, counted
 * directly. Reachability was established by searching every emission site in
 * `compiler/src` for each variant, excluding tests and excluding the dev-server
 * crate's separate enum of the same name. The fixes were then checked by
 * constructing a template that triggers the code and confirming the fix clears
 * it.
 */

export interface ErrorCodeDoc {
  code: string
  /** True when the compiler itself emits this code. */
  emitted: boolean
  severity: 'error' | 'warning' | 'mixed' | 'n/a'
  /** When it fires. Empty for codes the compiler never emits. */
  cause: string
  fix?: string
  wrong?: string
  right?: string
  note?: string
}

export const ERROR_CODES: ErrorCodeDoc[] = [
  {
    code: 'UnexpectedToken',
    emitted: true,
    severity: 'error',
    cause:
      'The default parse and lex failure. Also raised when a construct that looks like a block tag is not one: {with} is lexed as a block keyword but no parser supports it, so every {with} produces "Unknown block: {with}".',
    fix: 'Check the tag exists. The usable block tags are if, each, match, fetch, repeat, try, slot, layout, head, sections and blocks. {error} and {loading} are branches, legal only inside {fetch} or {try}. {with} does not exist in any form.',
    wrong: '{with user}${name}{/with}',
    right: '${ user.name }',
  },
  {
    code: 'UnclosedBlock',
    emitted: true,
    severity: 'error',
    cause: 'A block tag was opened and never closed, or an expression was left open.',
    fix: 'Add the closing tag. Note which tags need one and which do not: {each}, {match}, {fetch}, {repeat}, {try}, {layout}, {head} and {/if} all require a close, while {empty}, {else}, {else if}, {loading} and {error} are branch markers that take NO closing tag of their own.',
    wrong: '{each p in products}<li>${p.name}</li>{empty}<li>none</li>{/empty}{/each}',
    right: '{each p in products}<li>${p.name}</li>{empty}<li>none</li>{/each}',
  },
  {
    code: 'InvalidFrontmatter',
    emitted: true,
    severity: 'error',
    cause:
      'A malformed frontmatter directive or schema declaration. This is by far the largest family: a bad setting kind, a select default that is not one of its options, a duplicate setting key or block type, a section with no name, a max below 1, an empty @theme(), or any @target other than @theme.',
    fix: 'Read the message, which names the exact key or token. Two rules catch most of it: every setting kind must be one of text, richtext, color, image, icon, link, select, number or toggle; and a select default must appear in its own option list.',
    wrong: '---\nsection {\n  settings { align: select(left, center) = right }\n}\n---',
    right: '---\nsection {\n  name: "Hero"\n  settings { align: select(left, center, right) = right }\n}\n---',
    note: 'Unknown frontmatter STATEMENTS are silently skipped rather than reported, so a misspelled directive produces no diagnostic at all. This code fires only on a malformed one.',
  },
  {
    code: 'CircularImport',
    emitted: true,
    severity: 'error',
    cause: 'Two templates import each other, directly or through a chain, or the chain exceeds 128 links.',
    fix: 'Break the cycle by extracting the shared part into a third file that neither of the two imports back.',
    note: 'Only reachable through the virtual-files compile path. Neither the `slurp` CLI nor this MCP server detects it, because neither builds a cross-file import graph.',
  },
  {
    code: 'MissingFile',
    emitted: false,
    severity: 'n/a',
    cause: '',
    note: 'Declared for host integrations. The compiler never emits it; the dev server has its own separate code of the same name.',
  },
  {
    code: 'MissingRequiredProp',
    emitted: false,
    severity: 'n/a',
    cause: '',
    note: 'Declared but never emitted. Prop types and requiredness are recorded in frontmatter and are NOT enforced at render time, so a missing required prop is silently null.',
  },
  {
    code: 'UnknownProp',
    emitted: false,
    severity: 'n/a',
    cause: '',
    note: 'Declared but never emitted. Passing a prop a component never declared is not an error.',
  },
  {
    code: 'TypeMismatch',
    emitted: false,
    severity: 'n/a',
    cause: '',
    note: 'Declared but never emitted. Type annotations in frontmatter are stored as raw strings and never checked.',
  },
  {
    code: 'SecretEnvInTemplate',
    emitted: true,
    severity: 'error',
    cause:
      'A template read env.SLURP_SECRET_something, in dotted or subscript form. Also fires for env[key] with a key that is not a compile-time constant, because such a key could resolve to a secret name.',
    fix: 'A secret must never reach a template, since the template output is sent to the browser. Read it on the server and pass only the derived, non-secret value through the render context.',
    wrong: '<script>const k = ${ env.SLURP_SECRET_API_KEY | js };</script>',
    right: '<div data-configured={ integration.enabled }></div>',
  },
  {
    code: 'MiddlewareScopeViolation',
    emitted: true,
    severity: 'error',
    cause:
      'Either request.* was used in a file that is not compiled as middleware, or a file that IS middleware read a path root other than request, env or loop.',
    fix: 'request.* is middleware-only. In an ordinary page, take what you need from the request on the server and pass it through the render context. Note that "middleware" is a compile OPTION, not a filename convention, so the CLI and this server both treat every file as non-middleware unless told otherwise.',
    wrong: '{if request.cookies.session}<p>Welcome</p>{/if}',
    right: '{if user.signed_in}<p>Welcome</p>{/if}',
  },
  {
    code: 'IterationLimitExceeded',
    emitted: true,
    severity: 'mixed',
    cause:
      'A budget was exhausted: render depth, a single filter value over 8 MiB, cumulative filter work over 64 MiB, output over 16 MiB, a collection truncated at 1000 items, or the global 1000000-iteration budget.',
    fix: 'Page or pre-slice the data on the server. A budget TRUNCATES; it never aborts the render, so the page still ships with a quietly shortened list.',
    note: 'Truncation is a WARNING, not an error, in every mode: the loop keeps its first N items and the render succeeds. On the CLI the diagnostic is produced either way but only printed with -v, so a build that drops a list tail still exits 0 with a quiet console unless you ask.',
  },
  {
    code: 'MissingImageSrc',
    emitted: true,
    severity: 'error',
    cause: 'An <Image> was written with no src prop.',
    fix: 'Give it a src. There is no missing-dimension case: width and height default to 800 and 600 when absent, and both accept a literal such as width={400} as well as a context value.',
    wrong: '<Image alt="Product" width={400} height={300} />',
    right: '<Image src={product.image} alt="Product" width={400} height={300} />',
  },
  {
    code: 'LayoutDepthExceeded',
    emitted: false,
    severity: 'n/a',
    cause: '',
    note: 'Declared but never emitted UNDER THIS NAME. Layout nesting is bounded by the render depth limit, which reports IterationLimitExceeded instead.',
  },
  {
    code: 'UnknownComponent',
    emitted: false,
    severity: 'n/a',
    cause: '',
    note: 'Declared but never emitted. An unresolvable component renders a <div data-slurp-component="Name" data-slurp-props="..."> placeholder instead of failing, which is why an unresolved import looks like a working build.',
  },
  {
    code: 'UnknownSlot',
    emitted: false,
    severity: 'n/a',
    cause: '',
    note: 'Declared but never emitted. Only the slot names "default" and "head" are ever filled; any other name renders as a literal <slot name="x"> element.',
  },
  {
    code: 'UnknownFilter',
    emitted: true,
    severity: 'error',
    cause: 'A filter name that is in neither table. The value table and the loop table are disjoint, so using one where the other belongs also lands here.',
    fix: 'Value filters (usable in ${ }): upper, lower, currency, truncate, fixed, date, default, plural, int, float, js, json, unsafe_js. Loop filters (usable ONLY in an {each} header): limit, sort, reverse, filter.',
    wrong: '{each p in products | upper}<li>${p.name}</li>{/each}',
    right: '{each p in products | sort("name")}<li>${p.name}</li>{/each}',
    note: 'The value is passed through UNFILTERED alongside the diagnostic, so the page renders and looks nearly right.',
  },
  {
    code: 'InvalidFilterArgs',
    emitted: true,
    severity: 'error',
    cause: 'A required filter argument was missing or could not be read as a literal. In practice this is limit() and filter().',
    fix: 'Supply the argument as a literal. Filter arguments are read as literals throughout, so a variable is seen as the empty string.',
    wrong: '{each p in products | limit(pageSize)}<li>${p.name}</li>{/each}',
    right: '{each p in products | limit(12)}<li>${p.name}</li>{/each}',
  },
  {
    code: 'RedirectOutsideMiddleware',
    emitted: true,
    severity: 'error',
    cause: 'A {redirect} or {next} appeared in a file that is not compiled as middleware.',
    fix: 'Both are middleware-only signals. Move the check into a middleware file, or perform the redirect on the server before rendering.',
    wrong: '{if !user}{redirect "/login"}{/if}',
    right: 'Handle the redirect in middleware, and let the page assume it is reachable.',
  },
  {
    code: 'InfiniteLoop',
    emitted: false,
    severity: 'n/a',
    cause: '',
    note: 'Declared but never emitted. A component or block that renders itself is caught by a cycle guard that SKIPS silently, so a recursive include produces missing output rather than an error.',
  },
  {
    code: 'UnsafeScriptInterpolation',
    emitted: true,
    severity: 'mixed',
    cause:
      'Two distinct rules share this code, plus one CSS diagnostic. (1) An expression in a <script> body with neither | js nor | json. (2) A | js slot in a JavaScript-evaluated attribute that lands in statement or expression position rather than inside a quoted string. (3) As a development warning, CSS-structural characters stripped from a style value slot, and an undeclared slot in a JavaScript-evaluated attribute.',
    fix:
      'In a <script> body, end the expression with | js when it sits inside a string literal you wrote, or | json for a bare value. In a JS attribute, quote the slot if you want | js, or switch to | json where the slot stands alone. Use | unsafe_js only for JavaScript the theme itself authored.',
    wrong: '<script>var name = ${ user.name };</script>',
    right: '<script>var name = ${ user.name | json };</script>',
    note: 'HTML entity escaping does not protect a script context: a raw-text element reaches the JavaScript engine without character references being decoded, so the ordinary text escaping is inert there. Note also that a JavaScript template literal cannot be written in a <script> body at all, because Slurp claims the "${" sequence itself.',
  },
  {
    code: 'JsTemplateLiteralInAttribute',
    emitted: true,
    severity: 'warning',
    cause:
      'A quoted HTML attribute value contains a JavaScript template literal whose ${ } slot Slurp will interpolate itself, against the SERVER render context rather than the browser one.',
    fix: 'If the value is client-side, build the string by concatenation. If it is server-side data, drop the backticks and let Slurp interpolate.',
    wrong: '<a :href="`/product/${item.slug}`">View</a>',
    right: "<a :href=\"'/product/' + item.slug\">View</a>",
    note: 'This one was worth a dedicated code because of HOW it fails. The slot renders EMPTY, with no runtime error and no other sign, so the result is a page that looks built and has dead links and unstyled elements scattered through it. It is easy to ship and hard to notice, which is why it is diagnosed at parse time rather than left to review.',
  },
]

export const EMITTED_ERROR_CODES = ERROR_CODES.filter((e) => e.emitted)
export const DECLARED_ONLY_ERROR_CODES = ERROR_CODES.filter((e) => !e.emitted)
