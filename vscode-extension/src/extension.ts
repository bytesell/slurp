import * as vscode from 'vscode'
import { getDiagnostics } from '@slurp/language'
import type { SlurpDiagnostic } from '../../shared/interfaces'
import {
  SlurpCompletionProvider,
  SlurpHoverProvider,
  SlurpDefinitionProvider,
  SlurpFormattingProvider,
} from './providers'

let diagnosticCollection: vscode.DiagnosticCollection
const disposables: vscode.Disposable[] = []

export function activate(context: vscode.ExtensionContext): void {
  diagnosticCollection = vscode.languages.createDiagnosticCollection('slurp')
  context.subscriptions.push(diagnosticCollection)

  // Register completion provider
  disposables.push(
    vscode.languages.registerCompletionItemProvider(
      { language: 'slurp' },
      new SlurpCompletionProvider(),
      '|', '.', ' ', '{',
    ),
  )

  // Register hover provider
  disposables.push(
    vscode.languages.registerHoverProvider(
      { language: 'slurp' },
      new SlurpHoverProvider(),
    ),
  )

  // Register definition provider (Cmd+Click on PascalCase component name)
  disposables.push(
    vscode.languages.registerDefinitionProvider(
      { language: 'slurp' },
      new SlurpDefinitionProvider(),
    ),
  )

  // Register document formatting provider
  disposables.push(
    vscode.languages.registerDocumentFormattingEditProvider(
      { language: 'slurp' },
      new SlurpFormattingProvider(),
    ),
  )

  // Command: validate current file
  disposables.push(
    vscode.commands.registerCommand('slurp.validate', () => {
      const editor = vscode.window.activeTextEditor
      if (editor && editor.document.languageId === 'slurp') {
        validateDocument(editor.document)
      }
    }),
  )

  // Command: delegate to built-in format command
  disposables.push(
    vscode.commands.registerCommand('slurp.format', () => {
      vscode.commands.executeCommand('editor.action.formatDocument')
    }),
  )

  // Validate whichever .slurp file is already open when the extension activates
  if (vscode.window.activeTextEditor?.document.languageId === 'slurp') {
    validateDocument(vscode.window.activeTextEditor.document)
  }

  // Validate on change (debounced) when validateOnType is enabled
  disposables.push(
    vscode.workspace.onDidChangeTextDocument(e => {
      const config = vscode.workspace.getConfiguration('slurp')
      if (e.document.languageId === 'slurp' && config.get<boolean>('validateOnType')) {
        scheduleValidation(e.document)
      }
    }),
  )

  // Validate on save when validateOnSave is enabled
  disposables.push(
    vscode.workspace.onDidSaveTextDocument(doc => {
      const config = vscode.workspace.getConfiguration('slurp')
      if (doc.languageId === 'slurp' && config.get<boolean>('validateOnSave')) {
        validateDocument(doc)
      }
    }),
  )

  // Format on save when formatOnSave is enabled.
  //
  // `editor.action.formatDocument` must NOT be used here. It formats the ACTIVE
  // EDITOR rather than `e.document`, so on Save All with a dirty .slurp file in
  // a background tab it reformats whatever happens to be focused. That command
  // also resolves `undefined` and edits out of band, so its edits can land
  // AFTER the write that `waitUntil` is meant to gate.
  //
  // Formatting `e.document` directly and returning the TextEdit[] addresses
  // both: the right document is formatted, and `waitUntil` receives edits it
  // can actually apply before the file is written.
  disposables.push(
    vscode.workspace.onWillSaveTextDocument(e => {
      const config = vscode.workspace.getConfiguration('slurp')
      if (e.document.languageId !== 'slurp') return
      if (!config.get<boolean>('formatOnSave')) return

      e.waitUntil(computeFormatEdits(e.document))
    }),
  )

  context.subscriptions.push(...disposables)
}

/**
 * Formats `doc` with the registered formatter and returns the resulting edits.
 *
 * Asks the editor for the edits for THIS document (rather than running a
 * command against the active editor) so Save All formats each saved file with
 * its own content and its own formatter options.
 */
async function computeFormatEdits(doc: vscode.TextDocument): Promise<vscode.TextEdit[]> {
  try {
    const edits = await vscode.commands.executeCommand<vscode.TextEdit[]>(
      'vscode.executeFormatDocumentProvider',
      doc.uri,
    )
    return edits ?? []
  } catch {
    // A formatting failure must never block a save.
    return []
  }
}

export function deactivate(): void {
  diagnosticCollection.dispose()
  for (const d of disposables) d.dispose()
  disposables.length = 0
}

// ── Debounced validation ──────────────────────────────────────────────────────

const validationTimers = new Map<string, ReturnType<typeof setTimeout>>()

function scheduleValidation(doc: vscode.TextDocument): void {
  const key = doc.uri.toString()
  const existing = validationTimers.get(key)
  if (existing) clearTimeout(existing)
  validationTimers.set(
    key,
    setTimeout(() => {
      validationTimers.delete(key)
      validateDocument(doc)
    }, 300),
  )
}

function validateDocument(doc: vscode.TextDocument): void {
  // SECURITY: This only parses the document text - no code is executed.
  // getWasmValidator returns a parse-only function; execution is never invoked.
  const wasmValidate = getWasmValidator()
  getDiagnostics(doc.getText(), doc.uri.fsPath, wasmValidate)
    .then(errors => {
      const vsDiagnostics = errors.map(e => slurpErrorToVsDiagnostic(e, doc))
      diagnosticCollection.set(doc.uri, vsDiagnostics)
    })
    .catch(() => {
      // Never crash the extension on diagnostic errors - fail silently.
    })
}

function slurpErrorToVsDiagnostic(
  error: SlurpDiagnostic,
  doc: vscode.TextDocument,
): vscode.Diagnostic {
  // `doc.lineAt` THROWS on an out-of-range line. Because the whole set is built
  // with a single `.map`, one diagnostic pointing past the end of the document
  // (a stale error after an edit, or an off-by-one from a producer) would
  // reject the promise and silently drop EVERY diagnostic for the file. Clamp
  // instead: a slightly misplaced squiggle beats no squiggles at all.
  const lastLine = Math.max(0, doc.lineCount - 1)
  const line = Math.min(lastLine, Math.max(0, error.line - 1))
  const lineText = doc.lineAt(line).text
  const col = Math.min(lineText.length, Math.max(0, error.column - 1))
  const range = new vscode.Range(line, col, line, lineText.length)

  const severity =
    error.severity === 'error'
      ? vscode.DiagnosticSeverity.Error
      : error.severity === 'warning'
        ? vscode.DiagnosticSeverity.Warning
        : vscode.DiagnosticSeverity.Information

  const diagnostic = new vscode.Diagnostic(range, error.message, severity)
  diagnostic.code = error.code
  diagnostic.source = 'slurp'
  return diagnostic
}

/**
 * Returns the WASM compiler's parse-only validate function if the optional
 * `@slurp/compiler-wasm` package is installed.  Falls back to a no-op that
 * returns an empty array so the extension works without WASM.
 *
 * SECURITY: We only call `wasm.validate`, which is a parse-only operation.
 * We never call `wasm.render`, `wasm.execute`, or any code-execution entry
 * point.  Theme/template content is never eval'd.
 */
function getWasmValidator(): (source: string) => SlurpDiagnostic[] {
  try {
    // eslint-disable-next-line @typescript-eslint/no-require-imports
    const wasm = require('@slurp/compiler-wasm') as Record<string, unknown>
    if (typeof wasm['validate'] === 'function') {
      const validate = wasm['validate'] as (source: string) => unknown
      return (source: string): SlurpDiagnostic[] => {
        // The WASM exports serialize their return value as a JSON STRING
        // (JsValue::from_str in compiler/src/wasm/mod.rs), so it must be
        // parsed before use; a bare Array.isArray check never matches.
        let result = validate(source)
        if (typeof result === 'string') {
          try {
            result = JSON.parse(result) as unknown
          } catch {
            return []
          }
        }
        return Array.isArray(result) ? (result as SlurpDiagnostic[]) : []
      }
    }
  } catch {
    // @slurp/compiler-wasm not installed - return safe no-op.
  }
  return () => []
}
