import type { Plugin, Printer, AstPath, Doc, Options } from 'prettier'
import { builders } from 'prettier/doc'
import { formatSlurp, type EndOfLine } from '@bytesell/slurp-language/format'

const { hardline } = builders

export const plugin: Plugin = {
  languages: [
    {
      name: 'Slurp',
      parsers: ['slurp'],
      extensions: ['.slurp'],
      vscodeLanguageIds: ['slurp'],
    },
  ],
  parsers: {
    slurp: {
      parse(text: string) {
        return parseForFormat(text)
      },
      astFormat: 'slurp-ast',
      locStart: (node: FormatNode) => node.start,
      locEnd: (node: FormatNode) => node.end,
    },
  },
  printers: {
    'slurp-ast': {
      print(
        path: AstPath<FormatNode>,
        options: Options,
        _print: (path: AstPath<FormatNode>) => Doc,
      ): Doc {
        return printNode(path, options, _print)
      },
    } as Printer<FormatNode>,
  },
}

/** Internal AST node used only for formatting - not the compiler AST. */
interface FormatNode {
  type: string
  start: number
  end: number
  children?: FormatNode[]
  raw?: string
}

function parseForFormat(text: string): FormatNode {
  return {
    type: 'root',
    start: 0,
    end: text.length,
    raw: text,
  }
}

function printNode(
  path: AstPath<FormatNode>,
  options: Options,
  _print: (path: AstPath<FormatNode>) => Doc,
): Doc {
  const node = path.node
  if (!node.raw) return ''

  const tabWidth = options.tabWidth ?? 2
  const useTabs = options.useTabs ?? false

  // Prettier normalises line endings itself AFTER the printer runs, so the
  // printer must emit `\n` and let `endOfLine` be applied downstream. Emitting
  // CRLF here would be doubled into `\r\r\n`.
  const formatted = formatSlurp(node.raw, {
    indentUnit: useTabs ? '\t' : ' '.repeat(tabWidth),
    endOfLine: 'lf' satisfies EndOfLine,
  })

  const lines = formatted.split('\n')
  const docs: Doc[] = []
  for (let i = 0; i < lines.length; i++) {
    docs.push(lines[i] as Doc)
    if (i < lines.length - 1) docs.push(hardline)
  }
  return docs
}

export default plugin
