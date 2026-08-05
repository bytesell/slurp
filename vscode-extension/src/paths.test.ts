/**
 * `slurp.componentsPath` must not select a directory outside the workspace
 * folder.
 */

import { describe, test, expect } from 'vitest'
import { toWorkspaceRelative } from './paths'

describe('toWorkspaceRelative - accepted values', () => {
  test('the default resolves to the components directory', () => {
    expect(toWorkspaceRelative('./components')).toBe('components')
  })

  test('a bare relative directory is kept', () => {
    expect(toWorkspaceRelative('components')).toBe('components')
  })

  test('a nested relative directory is kept', () => {
    expect(toWorkspaceRelative('src/theme/components')).toBe('src/theme/components')
  })

  test('backslashes are normalised to forward slashes', () => {
    expect(toWorkspaceRelative('src\\components')).toBe('src/components')
  })

  test('trailing slashes are stripped', () => {
    expect(toWorkspaceRelative('components///')).toBe('components')
  })

  test('an empty value means the workspace root', () => {
    expect(toWorkspaceRelative('')).toBe('')
    expect(toWorkspaceRelative('.')).toBe('')
    expect(toWorkspaceRelative('./')).toBe('')
  })

  test('surrounding whitespace is ignored', () => {
    expect(toWorkspaceRelative('  components  ')).toBe('components')
  })

  test('a directory whose name merely starts with two dots is allowed', () => {
    expect(toWorkspaceRelative('..hidden')).toBe('..hidden')
  })
})

describe('toWorkspaceRelative - values that escape the workspace', () => {
  test('a POSIX absolute path is rejected', () => {
    expect(toWorkspaceRelative('/')).toBeNull()
    expect(toWorkspaceRelative('/etc')).toBeNull()
    expect(toWorkspaceRelative('/home/user/.ssh')).toBeNull()
  })

  test('a Windows drive-letter path is rejected', () => {
    // `path.resolve(workspace, 'C:\\')` returns 'C:\\', which is how a
    // recursive walk ends up reading the entire drive.
    expect(toWorkspaceRelative('C:\\')).toBeNull()
    expect(toWorkspaceRelative('C:\\Users')).toBeNull()
    expect(toWorkspaceRelative('c:/Users/someone')).toBeNull()
  })

  test('a UNC share is rejected', () => {
    expect(toWorkspaceRelative('\\\\server\\share')).toBeNull()
    expect(toWorkspaceRelative('//server/share')).toBeNull()
  })

  test('parent traversal is rejected', () => {
    expect(toWorkspaceRelative('..')).toBeNull()
    expect(toWorkspaceRelative('../..')).toBeNull()
    expect(toWorkspaceRelative('../../../etc')).toBeNull()
  })

  test('traversal buried mid-path is rejected', () => {
    expect(toWorkspaceRelative('components/../../../etc')).toBeNull()
    expect(toWorkspaceRelative('a/b/../../..')).toBeNull()
  })

  test('traversal written with backslashes is rejected', () => {
    expect(toWorkspaceRelative('..\\..\\secrets')).toBeNull()
  })

  test('a traversal that would still land inside is rejected too', () => {
    // `a/../b` is inside the workspace, but accepting it means shipping a
    // normaliser, and a partial normaliser is where traversal bugs live.
    expect(toWorkspaceRelative('a/../b')).toBeNull()
  })
})
