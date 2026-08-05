import type { SlurpDiagnostic } from '../../shared/interfaces'

/**
 * Run diagnostics on a .slurp source file.
 *
 * SECURITY: This only parses the source string - it does NOT execute any code.
 * The WASM compiler is invoked in parse-only mode; no eval or dynamic execution occurs.
 *
 * @param source - Raw text content of the .slurp file
 * @param filePath - Absolute file path, attached to each returned diagnostic
 * @param wasmValidate - Parse-only WASM function; must not execute template code
 * @returns Array of diagnostics with the filePath attached, or [] on parser crash
 */
export async function getDiagnostics(
  source: string,
  filePath: string,
  wasmValidate: (source: string) => SlurpDiagnostic[],
): Promise<SlurpDiagnostic[]> {
  try {
    return wasmValidate(source).map(d => ({ ...d, file: filePath }))
  } catch {
    // Parser errors must not crash the IDE - return empty to fail silently
    return []
  }
}
