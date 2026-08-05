import type * as Monaco from 'monaco-editor'
import type { SlurpDiagnostic } from '../../shared/interfaces'
import { getCompletions } from './completions'
import { getHover } from './hover'
import { getDiagnostics } from './diagnostics'

export function registerSlurp(
  monaco: typeof Monaco,
  wasmValidate?: (source: string) => SlurpDiagnostic[],
): void {
  monaco.languages.register({ id: 'slurp', extensions: ['.slurp'] })

  monaco.languages.registerCompletionItemProvider('slurp', {
    triggerCharacters: ['|', '.', ' ', '{'],
    provideCompletionItems(model, position) {
      const source = model.getValue()
      const completions = getCompletions(source, position.lineNumber, position.column)
      return {
        suggestions: completions.map(c => ({
          label: c.label,
          kind: mapCompletionKind(monaco, c.kind),
          detail: c.detail,
          documentation: c.documentation,
          insertText: c.insertText ?? c.label,
          insertTextRules: c.insertTextFormat === 'snippet'
            ? monaco.languages.CompletionItemInsertTextRule.InsertAsSnippet
            : undefined,
          range: {
            startLineNumber: position.lineNumber,
            startColumn: position.column,
            endLineNumber: position.lineNumber,
            endColumn: position.column,
          },
        })),
      }
    },
  })

  monaco.languages.registerHoverProvider('slurp', {
    provideHover(model, position) {
      const word = model.getWordAtPosition(position)
      if (!word) return null
      const hover = getHover(word.word)
      if (!hover) return null
      return {
        contents: [{ value: hover.contents }],
        range: hover.range ? {
          startLineNumber: position.lineNumber,
          startColumn: hover.range.start.character,
          endLineNumber: position.lineNumber,
          endColumn: hover.range.end.character,
        } : undefined,
      }
    },
  })

  if (wasmValidate) {
    const debounced = debounce(async (model: Monaco.editor.ITextModel) => {
      const diagnostics = await getDiagnostics(model.getValue(), model.uri.path, wasmValidate)
      const markers = diagnostics.map(d => ({
        severity: mapSeverity(monaco, d.severity),
        startLineNumber: d.line,
        startColumn: d.column,
        endLineNumber: d.line,
        endColumn: d.column + 10,
        message: d.message,
        code: d.code,
      }))
      monaco.editor.setModelMarkers(model, 'slurp', markers)
    }, 300)

    monaco.editor.onDidCreateModel(model => {
      if (model.getLanguageId() === 'slurp') {
        model.onDidChangeContent(() => debounced(model))
        debounced(model)
      }
    })
  }
}

function mapCompletionKind(monaco: typeof Monaco, kind: string): Monaco.languages.CompletionItemKind {
  const map: Record<string, Monaco.languages.CompletionItemKind> = {
    keyword: monaco.languages.CompletionItemKind.Keyword,
    snippet: monaco.languages.CompletionItemKind.Snippet,
    variable: monaco.languages.CompletionItemKind.Variable,
    property: monaco.languages.CompletionItemKind.Property,
    function: monaco.languages.CompletionItemKind.Function,
    class: monaco.languages.CompletionItemKind.Class,
    module: monaco.languages.CompletionItemKind.Module,
    value: monaco.languages.CompletionItemKind.Value,
  }
  return map[kind] ?? monaco.languages.CompletionItemKind.Text
}

function mapSeverity(monaco: typeof Monaco, severity: 'error' | 'warning' | 'info'): Monaco.MarkerSeverity {
  switch (severity) {
    case 'error': return monaco.MarkerSeverity.Error
    case 'warning': return monaco.MarkerSeverity.Warning
    case 'info': return monaco.MarkerSeverity.Info
  }
}

function debounce<T extends (...args: Parameters<T>) => void>(fn: T, ms: number): T {
  let timer: ReturnType<typeof setTimeout> | null = null
  return ((...args: Parameters<T>) => {
    if (timer) clearTimeout(timer)
    timer = setTimeout(() => fn(...args), ms)
  }) as T
}
