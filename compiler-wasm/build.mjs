#!/usr/bin/env node
/**
 * Builds the Slurp compiler to WebAssembly.
 *
 * WHY A SCRIPT AND NOT A BARE `wasm-pack` LINE IN package.json:
 *   1. `wasm-pack --out-dir` is resolved relative to the CRATE directory, not the
 *      cwd, so a literal relative path silently means different things depending
 *      on where it is invoked from. This passes an absolute path.
 *   2. The two things that can be missing (`wasm-pack` itself, and the
 *      `wasm32-unknown-unknown` rustup target) both fail with output that does
 *      not name the fix. Each is checked up front and reported with the exact
 *      command to run.
 *
 * TARGET CHOICE: `nodejs` (CommonJS + `fs.readFileSync` of the .wasm) is the
 * default because both of this repo's consumers are Node-side - the vitest
 * integration suite and the VS Code extension, which loads the package with
 * `require('@slurp/compiler-wasm')`. A `web`/`bundler` build is a flag away
 * (`node build.mjs --target web`) for a browser host that has a bundler.
 */
import { spawnSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import { dirname, join, resolve } from 'node:path'
import { existsSync, rmSync } from 'node:fs'
import { homedir } from 'node:os'

const here = dirname(fileURLToPath(import.meta.url))
const crateDir = resolve(here, '..', 'compiler')
const outDir = join(here, 'pkg')

const VALID_TARGETS = ['nodejs', 'web', 'bundler', 'no-modules', 'deno']
const targetArgIndex = process.argv.indexOf('--target')
const target = targetArgIndex === -1 ? 'nodejs' : process.argv[targetArgIndex + 1]

if (!VALID_TARGETS.includes(target)) {
  console.error(
    `[compiler-wasm] Unknown --target "${target}". Valid targets: ${VALID_TARGETS.join(', ')}`,
  )
  process.exit(1)
}

/**
 * Run a command, returning its exit status without throwing on a missing binary.
 *
 * WINDOWS: a cargo-installed `wasm-pack` is a real .exe and spawns directly, but
 * an npm-installed one is a `.cmd` shim that only resolves through the shell.
 * Try the direct spawn first and fall back, rather than always setting
 * `shell: true` - passing an args array with `shell: true` is a Node
 * deprecation warning (DEP0190) and quotes nothing, so any path containing a
 * space would break.
 */
function spawn(cmd, args, opts) {
  const direct = spawnSync(cmd, args, opts)
  if (direct.error?.code === 'ENOENT' && process.platform === 'win32') {
    return spawnSync(cmd, args, { ...opts, shell: true })
  }
  return direct
}

function run(cmd, args, env) {
  return spawn(cmd, args, { stdio: 'inherit', ...(env ? { env } : {}) })
}

function capture(cmd, args) {
  return spawn(cmd, args, { encoding: 'utf8' })
}

// ── Preflight: wasm-pack ─────────────────────────────────────────────────────
const versionProbe = capture('wasm-pack', ['--version'])
if (versionProbe.error || versionProbe.status !== 0) {
  console.error(
    [
      '[compiler-wasm] `wasm-pack` was not found on PATH.',
      '',
      'Install it with one of:',
      '  cargo install wasm-pack --locked',
      '  npm install -g wasm-pack',
      '  (see https://rustwasm.github.io/wasm-pack/installer/)',
    ].join('\n'),
  )
  process.exit(1)
}
console.log(`[compiler-wasm] using ${versionProbe.stdout.trim()}`)

// ── Preflight: the wasm32 rustup target ──────────────────────────────────────
// Without it `cargo build` fails partway through with a "can't find crate for
// `core`" error that says nothing about rustup.
const targets = capture('rustup', ['target', 'list', '--installed'])
if (!targets.error && targets.status === 0 && !targets.stdout.includes('wasm32-unknown-unknown')) {
  console.log('[compiler-wasm] installing the wasm32-unknown-unknown target...')
  const add = run('rustup', ['target', 'add', 'wasm32-unknown-unknown'])
  if (add.status !== 0) {
    console.error(
      '[compiler-wasm] `rustup target add wasm32-unknown-unknown` failed. Run it manually and retry.',
    )
    process.exit(1)
  }
}

// ── Build ────────────────────────────────────────────────────────────────────
// A stale pkg/ from a previous target would leave a mix of module systems
// behind, which fails at import time rather than at build time.
if (existsSync(outDir)) rmSync(outDir, { recursive: true, force: true })

const args = [
  'build',
  crateDir,
  '--target',
  target,
  '--out-dir',
  outDir,
  '--out-name',
  'slurp_compiler',
  // Release, because this artifact is what a browser or editor host downloads.
  '--release',
]

// Rust bakes the absolute source path of every panic site into the binary, so an unremapped
// build ships the builder's home directory - `C:\Users\<name>\.cargo\registry\...` - to every
// consumer of the published package. Remapping both roots makes the artifact independent of who
// built it, which also means two machines can produce byte-identical output.
const cargoHome = process.env['CARGO_HOME'] || join(homedir(), '.cargo')
const remap = [
  `--remap-path-prefix=${cargoHome}=/cargo`,
  `--remap-path-prefix=${resolve(here, '..')}=/slurp`,
]
const buildEnv = {
  ...process.env,
  RUSTFLAGS: [process.env['RUSTFLAGS'], ...remap].filter(Boolean).join(' '),
}

console.log(`[compiler-wasm] wasm-pack ${args.join(' ')}`)
const build = run('wasm-pack', args, buildEnv)
if (build.status !== 0) {
  console.error('[compiler-wasm] wasm-pack build failed.')
  process.exit(build.status ?? 1)
}

if (!existsSync(join(outDir, 'slurp_compiler_bg.wasm'))) {
  console.error('[compiler-wasm] build reported success but produced no .wasm. Aborting.')
  process.exit(1)
}

// wasm-pack writes a `pkg/.gitignore` containing `*`. npm reads `.gitignore` per directory, so
// leaving it there excludes the whole build output from the tarball even though `files` lists it -
// see the note in `.npmignore`. Root `.gitignore` still keeps `pkg/` out of git.
const pkgGitignore = join(outDir, '.gitignore')
if (existsSync(pkgGitignore)) rmSync(pkgGitignore)

console.log(`[compiler-wasm] built ${target} package at ${outDir}`)
