import { clsx, type ClassValue } from 'clsx'
import { twMerge } from 'tailwind-merge'

/**
 * Combines class names using clsx and resolves Tailwind CSS conflicts via
 * tailwind-merge. Conflicts are resolved in favour of the last class listed,
 * matching Tailwind's own precedence rules.
 *
 * Called automatically by `class={}` compilation output. Also exposed as
 * `window.cn` for use in inline Alpine expressions.
 *
 * @param inputs - Any combination of strings, arrays, objects, and falsy values
 *   accepted by clsx.
 * @returns A single, deduplicated class string with no Tailwind conflicts.
 *
 * @example
 * cn('px-4 py-2', isActive && 'bg-blue-500', 'px-6')
 * // => 'py-2 bg-blue-500 px-6'  (px-4 is overridden by px-6)
 */
export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs))
}
