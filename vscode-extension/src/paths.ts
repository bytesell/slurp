/**
 * Path containment for `slurp.componentsPath`.
 *
 * Deliberately free of any `vscode` import so the security gate can be tested
 * directly, without an extension host.
 */

/**
 * Normalises `slurp.componentsPath` into a workspace-relative glob base.
 *
 * `slurp.componentsPath` is a plain string setting, so a cloned repository can
 * set it from its own `.vscode/settings.json`. Passing it straight to
 * `path.resolve(workspaceRoot, componentsPath)` is unsafe, because
 * `path.resolve` IGNORES its base when the second argument is absolute: a value
 * of `"C:\\"`, `"/"` or `"\\\\server\\share"` escapes the workspace entirely,
 * and a directory walk rooted there reads the whole drive. `"../.."` escapes
 * the same way by relative traversal.
 *
 * Rejecting rather than silently re-basing is deliberate: re-basing would make
 * the setting quietly do something other than what it says.
 *
 * @param componentsPath - The raw setting value.
 * @returns A workspace-relative base (`''` meaning the workspace root), or null
 *   when the value escapes the workspace and must not be used.
 */
export function toWorkspaceRelative(componentsPath: string): string | null {
  let p = componentsPath.trim().replace(/\\/g, '/')

  // UNC share (`\\server\share`), POSIX absolute, or a Windows drive letter.
  if (p.startsWith('//')) return null
  if (p.startsWith('/')) return null
  if (/^[A-Za-z]:/.test(p)) return null

  // Strip a leading `./` and any trailing slashes.
  while (p.startsWith('./')) p = p.slice(2)
  p = p.replace(/\/+$/, '')

  if (p === '' || p === '.') return ''

  const segments = p.split('/').filter(seg => seg !== '' && seg !== '.')

  // Any `..` escapes the base. Reject outright instead of trying to resolve it:
  // `a/../../b` normalises to something outside the workspace, and a partial
  // normaliser is exactly where traversal bugs live.
  if (segments.some(seg => seg === '..')) return null

  if (segments.length === 0) return ''

  return segments.join('/')
}
