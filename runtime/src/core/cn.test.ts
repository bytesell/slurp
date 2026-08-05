import { describe, it, expect } from 'vitest'
import { cn } from './cn'

describe('cn', () => {
  it('returns a single class unchanged', () => {
    expect(cn('px-4')).toBe('px-4')
  })

  it('joins multiple classes', () => {
    expect(cn('px-4', 'py-2')).toBe('px-4 py-2')
  })

  it('ignores falsy values', () => {
    expect(cn('px-4', false, null, undefined, '')).toBe('px-4')
  })

  it('resolves Tailwind conflicts - last wins', () => {
    // px-4 and px-6 conflict; the latter should win.
    expect(cn('px-4', 'px-6')).toBe('px-6')
  })

  it('keeps non-conflicting utilities from both arguments', () => {
    expect(cn('py-2 px-4', 'px-6')).toBe('py-2 px-6')
  })

  it('accepts an object with conditional keys', () => {
    expect(cn({ 'bg-blue-500': true, 'bg-red-500': false })).toBe('bg-blue-500')
  })

  it('accepts an array of class values', () => {
    expect(cn(['px-4', 'py-2'])).toBe('px-4 py-2')
  })

  it('handles deeply nested arrays', () => {
    expect(cn(['px-4', ['py-2', 'text-sm']])).toBe('px-4 py-2 text-sm')
  })

  it('returns empty string when all inputs are falsy', () => {
    expect(cn(false, null, undefined)).toBe('')
  })

  it('resolves padding conflicts across separate arguments', () => {
    expect(cn('p-4', 'px-2')).toBe('p-4 px-2')
    // px-2 is more specific than p-4's x-axis; tailwind-merge keeps both.
    // Verify the result is deterministic regardless of our expectation.
    const result = cn('p-4', 'px-2')
    expect(typeof result).toBe('string')
  })

  it('resolves text colour conflicts', () => {
    expect(cn('text-red-500', 'text-blue-500')).toBe('text-blue-500')
  })

  it('handles empty input', () => {
    expect(cn()).toBe('')
  })
})
