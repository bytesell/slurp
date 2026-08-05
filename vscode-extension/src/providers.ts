import * as vscode from 'vscode'
import { getCompletions, getHover } from '@bytesell/slurp-language'
import { formatSlurp } from '@bytesell/slurp-language/format'
import { toWorkspaceRelative } from './paths'
import type { SlurpCompletionItem } from '../../shared/interfaces'

export class SlurpCompletionProvider implements vscode.CompletionItemProvider {
  provideCompletionItems(
    document: vscode.TextDocument,
    position: vscode.Position,
  ): vscode.CompletionItem[] {
    const source = document.getText()
    const completions = getCompletions(source, position.line + 1, position.character + 1)
    return completions.map(c => toVsCompletion(c))
  }
}

function toVsCompletion(c: SlurpCompletionItem): vscode.CompletionItem {
  const item = new vscode.CompletionItem(c.label, mapKind(c.kind))
  item.detail = c.detail
  item.documentation = c.documentation
    ? new vscode.MarkdownString(c.documentation)
    : undefined
  if (c.insertText) {
    item.insertText = c.insertTextFormat === 'snippet'
      ? new vscode.SnippetString(c.insertText)
      : c.insertText
  }
  if (c.sortText) item.sortText = c.sortText
  return item
}

function mapKind(kind: string): vscode.CompletionItemKind {
  const map: Record<string, vscode.CompletionItemKind> = {
    keyword: vscode.CompletionItemKind.Keyword,
    snippet: vscode.CompletionItemKind.Snippet,
    variable: vscode.CompletionItemKind.Variable,
    property: vscode.CompletionItemKind.Property,
    function: vscode.CompletionItemKind.Function,
    class: vscode.CompletionItemKind.Class,
    module: vscode.CompletionItemKind.Module,
    value: vscode.CompletionItemKind.Value,
  }
  return map[kind] ?? vscode.CompletionItemKind.Text
}

export class SlurpHoverProvider implements vscode.HoverProvider {
  provideHover(
    document: vscode.TextDocument,
    position: vscode.Position,
  ): vscode.Hover | null {
    const word = document.getWordRangeAtPosition(position)
    if (!word) return null
    const text = document.getText(word)
    const result = getHover(text)
    if (!result) return null
    return new vscode.Hover(new vscode.MarkdownString(result.contents))
  }
}

export class SlurpDefinitionProvider implements vscode.DefinitionProvider {
  async provideDefinition(
    document: vscode.TextDocument,
    position: vscode.Position,
    token?: vscode.CancellationToken,
  ): Promise<vscode.Definition | null> {
    const word = document.getWordRangeAtPosition(position)
    if (!word) return null
    const text = document.getText(word)

    // Only handle PascalCase names (components)
    if (!/^[A-Z][a-zA-Z0-9]*$/.test(text)) return null

    return findComponentFile(document, text, token)
  }
}

async function findComponentFile(
  document: vscode.TextDocument,
  componentName: string,
  token?: vscode.CancellationToken,
): Promise<vscode.Location | null> {
  const config = vscode.workspace.getConfiguration('slurp')
  const componentsPath = config.get<string>('componentsPath') ?? './components'

  const workspaceFolder = vscode.workspace.getWorkspaceFolder(document.uri)
  if (!workspaceFolder) return null

  // A relative glob base keeps the search inside the workspace folder even if
  // the containment check below were ever bypassed: `findFiles` will not follow
  // a RelativePattern outside its base.
  const relativeBase = toWorkspaceRelative(componentsPath)
  if (relativeBase === null) return null

  const pattern = new vscode.RelativePattern(
    workspaceFolder,
    relativeBase === '' ? `**/${componentName}.slurp` : `${relativeBase}/**/${componentName}.slurp`,
  )

  // `findFiles` is asynchronous, respects the editor's file watcher excludes,
  // and takes a CancellationToken. A SYNCHRONOUS
  // `fs.readdirSync(dir, { recursive: true })` on the extension host thread is
  // the shape to avoid here: uncached, uncancellable, and run on every
  // Ctrl-hover.
  const matches = await vscode.workspace.findFiles(pattern, '**/node_modules/**', 1, token)
  const first = matches[0]
  if (!first) return null

  return new vscode.Location(first, new vscode.Position(0, 0))
}


export class SlurpFormattingProvider implements vscode.DocumentFormattingEditProvider {
  provideDocumentFormattingEdits(
    document: vscode.TextDocument,
    options: vscode.FormattingOptions,
  ): vscode.TextEdit[] {
    try {
      const source = document.getText()
      const formatted = formatDocument(source, options, document.eol)

      // Returning no edit when nothing changed keeps the document off the
      // dirty list, so format-on-save cannot mark a file modified by saving it.
      if (formatted === source) return []

      const fullRange = new vscode.Range(
        document.positionAt(0),
        document.positionAt(source.length),
      )
      return [vscode.TextEdit.replace(fullRange, formatted)]
    } catch {
      // Never crash the extension on formatting errors.
      return []
    }
  }
}

/**
 * Formats a document with the SHARED formatter.
 *
 * A second, independent implementation here is what this deliberately avoids.
 * A dedent rule of `/^\{\/(\w+)\}/` (any close tag) differs from the Prettier
 * plugin's fixed keyword list, and on the `{fetch}` shape the extension's own
 * snippet inserts, the two disagree permanently - format-on-save and
 * `prettier --check` then fight each other on every save. There is exactly one
 * implementation, and it stays that way.
 *
 * @param eol - The document's line ending, preserved rather than normalised.
 */
function formatDocument(
  source: string,
  options: vscode.FormattingOptions,
  eol: vscode.EndOfLine,
): string {
  return formatSlurp(source, {
    indentUnit: options.insertSpaces ? ' '.repeat(options.tabSize) : '\t',
    // An editor must not silently rewrite a file's line endings on save, so the
    // document's existing EOL wins here (Prettier passes 'lf' on its side
    // because Prettier applies its own endOfLine option afterwards).
    endOfLine: eol === vscode.EndOfLine.CRLF ? 'crlf' : 'lf',
  })
}
