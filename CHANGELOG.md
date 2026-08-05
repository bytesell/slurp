# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

While the version is below 1.0, a minor version bump may contain breaking
changes to both the Rust API and the template language.

## [Unreleased]

### Added

- `slurp gitignore` writes a `.gitignore` for a theme, with `--append`,
  `--force` and `--stdout`. It refuses to overwrite an existing file.

### Changed

- **BREAKING (pre-1.0):** the error code `MissingImageDimension` is renamed
  `MissingImageSrc`. It only ever fired for an `<Image>` with no `src`; `width`
  and `height` are optional and default to 800 and 600, so the old name
  described a case that does not exist. Hosts switching on the code string need
  to update.

### Fixed

- The dev server never sent its `clear` HMR message, so an error overlay was
  removed only by the full page reload that a successful rebuild happens to
  trigger. A rebuild that fixed the error but produced no page left a stale
  overlay covering a document that now compiles.
- `slurp build` no longer writes `components/` and `layouts/` as pages. They
  were emitted as standalone HTML fragments carrying literal `<slot>` elements:
  on the example theme, ten junk files beside six real pages. They are still
  compiled and checked, so an error in a component no page imports yet still
  fails the build.
- The `--globals` file is no longer copied into the output. It is build input,
  and the static-asset pass was publishing it alongside the site.
- Loop truncation is reported on a production build. Both iteration budgets
  recorded their warning in development mode only, and the CLI always builds
  production, so `slurp build` dropped items past the cap with no diagnostic and
  `-v` could not surface one. Truncation behaviour is unchanged; only the
  silence is gone.

### Changed

- **A loop truncated at the 1,000-item cap is now reported in every build mode**,
  not only in development. `slurp build -v` prints
  `warning[IterationLimitExceeded]: Iteration limit: collection has N items,
  capped at 1000`. The truncation itself is unchanged: the render still keeps the
  first 1,000 items, still succeeds, and the output is byte-identical. Only the
  silence is gone. Truncation removes rows without leaving any trace in the
  markup, so a production render that said nothing reported a clean build while
  dropping part of a list. This matches the output-byte and render-depth budgets,
  which already warned in both modes. The WASM `render` export now returns this
  warning as well as `render_dev`.

### Removed

- `slurp_image::ImageCache::max_disk_bytes()`. The budget is supplied to
  `ImageCache::new` and to `sweep`, so the accessor had no callers.
- The unconstructed `Token::TemplateLiteralEnd` lexer token, an alias for
  `Token::TemplateLiteralClose` that no lexer path produced.

### Fixed

- The dev server's source-directory watcher no longer panics on a `--source-dir`
  it cannot watch, or when `current_exe()` fails because the binary is being
  rebuilt. It ran on a bare thread, so a panic there killed hot reload while the
  server carried on serving, with nothing logged. It now shares the theme
  watcher's implementation, which logs and stops.
- `slurp-dev-server` passes the source and output directories to `slurp build` as
  OS strings. A path that is not valid UTF-8 previously fell back to `.` and
  `dist`, building a different directory and reporting success.
- `slurp-image` sizes its cache from `Config` rather than directly from the
  command-line flags, so `Config::memory_cache_size` is honoured.

## [0.1.0] - 2026-08-01

First public release. Slurp was extracted from a private monorepo where it
renders multi-tenant sites.

### Added

- **Compiler** (`slurp-compiler`): lexer, recursive-descent parser and
  single-pass HTML renderer for the `.slurp` template language. Tags: `{if}`,
  `{each}`, `{match}`, `{repeat}`, `{fetch}`, `{try}`, `{html}`, `{$let}`,
  `{debug}`, `{slot}`, `{layout}`, `{head}`, `{sections}`, `{blocks}`,
  `{redirect}`, `{next}`. 13 value filters and 4 loop filters.
- **CLI** (`slurp`): `build`, `validate` and `dev`.
- **WASM bindings**: `parse`, `render`, `render_dev`, `validate`,
  `validate_full`, `extract_schema` and `render_section`. All but
  `validate_full` are for editor previews and skip the compile-time security
  walk; `validate_full` runs it, for a tool that needs parity with
  `slurp build`.
- **Editor schemas**: `section { }` and `block { }` frontmatter declaring an
  editable surface, with nine setting kinds, `@theme` block targeting, and
  tolerant merge rules that clamp and drop rather than reject.
- **Dev server** (`slurp-dev-server`): file watching, WebSocket hot reload,
  JSON fixtures for backendless previews, and an optional backend proxy.
- **Image service** (`slurp-image`): on-demand width resize and transcode to
  AVIF, WebP or JPEG, with a two-level cache.
- **Browser runtime** (`@slurp/runtime`): optional ESM bundles for client
  navigation, `{fetch}` data loading and infinite scroll.
- **Editor tooling**: TextMate grammar, completions, hover documentation, a
  Prettier plugin and a VS Code extension.

### Security

- Expression output is escaped for the context it lands in: HTML entities in
  text and plain attributes, a JS string escaper or a self-delimiting JSON
  literal in a JavaScript-evaluated attribute (chosen by the slot's position),
  removal of CSS-structural characters in a `style` value, and the ammonia
  allowlist sanitizer in `srcdoc`. `{html expr sanitize}` runs the same
  sanitizer.
- An interpolation in a `<script>` body is a **compile error** unless it
  declares its context with `| js` or `| json`, rather than being passed to an
  escaper that does not fit. A `| js` that is false about its position is a
  compile error too. Both rules have render-time backstops.
- URL attributes are scheme-checked, with whitespace, zero-width and bidi
  normalisation first, so obfuscated `javascript:` URLs are caught.
- Compile-time gates on `env.SLURP_SECRET_*` (dotted and subscript forms, in
  every file including middleware), `request.*` scope, `{redirect}` and
  `{next}`.
- Per-render budgets on iterations, filter memory, output size and recursion
  depth.
- The image service resolves DNS up front, checks every returned address against
  private and reserved ranges, pins those addresses into the HTTP client to
  defeat rebinding, disables redirects, and validates magic bytes.
- Known gaps are documented rather than implied away. See
  [SECURITY.md](./SECURITY.md), in particular the section on contexts where
  `${ }` escaping is not sufficient.

[Unreleased]: https://github.com/bytesell/slurp/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/bytesell/slurp/releases/tag/v0.1.0
