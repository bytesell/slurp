#!/usr/bin/env node
/**
 * Fails if an em-dash, en-dash or horizontal-ellipsis character appears in any
 * tracked text file.
 *
 * The project writes a hyphen for ranges and three literal dots for an
 * ellipsis, everywhere: source, comments, documentation, templates, commit-
 * ready copy. Those three code points are visually near-identical to their
 * ASCII spellings, so drift is invisible in review and is only caught
 * mechanically.
 *
 * The file set is every file git would consider part of the repository:
 * tracked files plus untracked ones that are not ignored. Ignored build output
 * (target/, node_modules/, pkg/, dist/) is therefore out of scope without
 * needing its own rule here, while a newly added file is checked before it is
 * ever committed. Binary files are skipped by extension and by a NUL-byte
 * check.
 *
 * The forbidden characters are written below as escapes so that this file does
 * not trip its own check.
 */

import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { extname } from 'node:path';

const FORBIDDEN = [
  { name: 'em-dash', char: String.fromCharCode(0x2014), instead: 'a hyphen (-), a comma, parentheses, or two sentences' },
  { name: 'en-dash', char: String.fromCharCode(0x2013), instead: 'a hyphen (-), including in ranges such as 15-35' },
  { name: 'ellipsis', char: String.fromCharCode(0x2026), instead: 'three literal dots (...)' },
];

/**
 * Files permitted to contain the characters, each with the reason.
 *
 * Keep this list empty where possible. An entry is a hole in the rule, so it
 * needs a reason that could not be met by spelling the character as an escape.
 */
const ALLOWED = new Map([
  [
    'mcp/test/llms.test.ts',
    'asserts the same property over the generated llms files and needs the literal characters to search for',
  ],
]);

const BINARY_EXTENSIONS = new Set([
  '.png', '.jpg', '.jpeg', '.gif', '.webp', '.avif', '.ico', '.bmp',
  '.woff', '.woff2', '.ttf', '.otf', '.eot',
  '.wasm', '.zip', '.gz', '.vsix', '.pdf', '.mp4', '.webm',
]);

function repositoryFiles() {
  const out = execFileSync(
    'git',
    ['ls-files', '-z', '--cached', '--others', '--exclude-standard'],
    { encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 },
  );
  return [...new Set(out.split('\0').filter(Boolean))];
}

function lineAndColumn(text, index) {
  const before = text.slice(0, index);
  const line = before.split('\n').length;
  const column = index - (before.lastIndexOf('\n') + 1) + 1;
  return { line, column };
}

const violations = [];
let scanned = 0;

for (const file of repositoryFiles()) {
  if (ALLOWED.has(file)) continue;
  if (BINARY_EXTENSIONS.has(extname(file).toLowerCase())) continue;

  let buffer;
  try {
    buffer = readFileSync(file);
  } catch {
    // A path in the index that is not present in the working tree, e.g. during
    // a partial checkout. Nothing to check.
    continue;
  }
  if (buffer.includes(0)) continue;

  const text = buffer.toString('utf8');
  scanned += 1;

  for (const { name, char, instead } of FORBIDDEN) {
    let at = text.indexOf(char);
    while (at !== -1) {
      const { line, column } = lineAndColumn(text, at);
      violations.push({ file, line, column, name, instead });
      at = text.indexOf(char, at + 1);
    }
  }
}

if (violations.length > 0) {
  console.error(`Copy rule: ${violations.length} forbidden character(s).\n`);
  for (const v of violations) {
    console.error(`  ${v.file}:${v.line}:${v.column}  ${v.name}  ->  use ${v.instead}`);
  }
  console.error('\nSee CONTRIBUTING.md. Replace the character; do not add an exception.');
  process.exit(1);
}

console.log(`Copy rule: clean (${scanned} text files scanned).`);
