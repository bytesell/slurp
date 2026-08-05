use std::path::{Path, PathBuf};
use std::process;

use clap::{Parser, Subcommand};
use serde_json::Value;

use slurp_compiler::{
    BuildMode, ErrorAccumulator, check_security, compile_with_registry, parser::parse,
};

#[derive(Parser)]
#[command(
    name = "slurp",
    about = "Slurp template compiler",
    version = env!("CARGO_PKG_VERSION"),
    author = "ByteSell LLC"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Build the theme for production.
    ///
    /// Compiles all `.slurp` files in the input directory tree,
    /// writing HTML output to the output directory.
    Build {
        /// Input directory (default: current directory)
        #[arg(short, long, default_value = ".")]
        input: PathBuf,

        /// Output directory (default: dist/)
        #[arg(short, long, default_value = "dist")]
        output: PathBuf,

        /// Path to a JSON file of build-time globals, seeded into the render context.
        ///
        /// Real page data still comes from the runtime; this is for values a theme must resolve at
        /// COMPILE time because they end up in a static attribute - notably the platform-owned
        /// runtime URLs (`${ platform.alpine }`), which the theme is forbidden from hardcoding.
        /// Without it a locally-built theme renders those attributes empty and the page loads no JS.
        #[arg(long)]
        globals: Option<PathBuf>,

        /// Directory whose `.slurp` files are middleware. Repeatable, relative to the input
        /// directory. See the same flag on `validate`.
        ///
        /// Middleware is checked and then SKIPPED: it produces no HTML, because a middleware file
        /// runs per request in the host and is not a page.
        #[arg(long = "middleware", value_name = "DIR")]
        middleware: Vec<PathBuf>,

        /// Emit verbose output
        #[arg(short, long)]
        verbose: bool,
    },

    /// Validate all `.slurp` files without producing output.
    ///
    /// Exits with status 1 if any errors are found.
    Validate {
        /// Directory to scan (default: current directory)
        #[arg(short, long, default_value = ".")]
        dir: PathBuf,

        /// Directory whose `.slurp` files are middleware. Repeatable, relative to the scanned
        /// directory: `--middleware middleware --middleware auth/guards`.
        ///
        /// Middleware obeys inverted scope rules - `request.*`, `{redirect}` and `{next}` are legal
        /// ONLY there, and everything else is illegal there - so the checker has to be told which
        /// files those are. It is a compile option rather than a path convention (a host may put
        /// middleware anywhere), and the CLI has no other way to learn it. Without this flag a
        /// correct middleware file reports `MiddlewareScopeViolation` and a broken one is never
        /// checked at all.
        #[arg(long = "middleware", value_name = "DIR")]
        middleware: Vec<PathBuf>,

        /// Show warnings in addition to errors
        #[arg(short, long)]
        warnings: bool,
    },

    /// Start the development server.
    Dev {
        /// Port to listen on
        #[arg(short, long, default_value = "3000")]
        port: u16,

        /// Bind address
        #[arg(short, long, default_value = "127.0.0.1")]
        bind: String,

        /// Path to a fixtures file (or a directory containing `fixtures.json`) mapping request
        /// path -> canned JSON response, so the theme previews with no backend running.
        #[arg(long)]
        fixtures: Option<PathBuf>,
    },

    /// Write a `.gitignore` for a Slurp theme.
    ///
    /// The template's most important content is what it tells you NOT to ignore: a theme's
    /// compiled stylesheet is committed alongside its source, and ignoring it produces a theme
    /// that publishes and renders as unstyled HTML with every static check still green.
    Gitignore {
        /// Directory to write into (default: current directory)
        #[arg(short, long, default_value = ".")]
        path: PathBuf,

        /// Append to an existing `.gitignore` instead of refusing.
        #[arg(long, conflicts_with = "force")]
        append: bool,

        /// Overwrite an existing `.gitignore`.
        #[arg(long)]
        force: bool,

        /// Print to stdout instead of writing a file.
        #[arg(long)]
        stdout: bool,
    },
}

/// The theme `.gitignore` template.
///
/// This const is the source of truth; `examples/basic-theme/.gitignore` is a copy of it, and a
/// test asserts the two match whenever that file is present. Embedding it rather than reading the
/// example keeps `slurp gitignore` working from a published crate, where `examples/` is not
/// shipped.
const THEME_GITIGNORE: &str = include_str!("theme.gitignore");

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::Build {
            input,
            output,
            globals,
            middleware,
            verbose,
        } => {
            cmd_build(&input, &output, globals.as_deref(), &middleware, verbose);
        }
        Commands::Validate {
            dir,
            middleware,
            warnings,
        } => {
            cmd_validate(&dir, &middleware, warnings);
        }
        Commands::Dev {
            port,
            bind,
            fixtures,
        } => {
            cmd_dev(&bind, port, fixtures.as_deref());
        }
        Commands::Gitignore {
            path,
            append,
            force,
            stdout,
        } => {
            cmd_gitignore(&path, append, force, stdout);
        }
    }
}

/// Write the theme `.gitignore` template.
///
/// Refusing to clobber is the default because the likely caller is someone running this in a
/// directory that already has one, and silently replacing a file full of a project's own rules
/// would be worse than doing nothing. `--append` and `--force` name the two things you might
/// actually want, and the refusal message says so rather than just failing.
fn cmd_gitignore(path: &Path, append: bool, force: bool, stdout: bool) {
    if stdout {
        print!("{THEME_GITIGNORE}");
        return;
    }

    if !path.is_dir() {
        eprintln!("Not a directory: {}", path.display());
        process::exit(1);
    }

    let target = path.join(".gitignore");
    let exists = target.exists();

    if exists && !append && !force {
        eprintln!(
            "{} already exists.\n\n  --append   add these rules to the end of it\n  --force    replace it\n  --stdout   print the template and write nothing",
            target.display()
        );
        process::exit(1);
    }

    let result = if append && exists {
        // Separated by a blank line so the appended block does not run into whatever the last
        // rule was, and headed so a reader can see where it came from.
        std::fs::read_to_string(&target)
            .map(|existing| {
                let sep = if existing.ends_with('\n') { "" } else { "\n" };
                format!("{existing}{sep}\n# ── Added by `slurp gitignore` ──\n{THEME_GITIGNORE}")
            })
            .and_then(|merged| std::fs::write(&target, merged))
    } else {
        std::fs::write(&target, THEME_GITIGNORE)
    };

    match result {
        Ok(()) => {
            let verb = if append && exists {
                "Appended to"
            } else if exists {
                "Replaced"
            } else {
                "Wrote"
            };
            println!("{verb} {}", target.display());
        }
        Err(e) => {
            eprintln!("Failed to write {}: {e}", target.display());
            process::exit(1);
        }
    }
}

/// Read a `--globals` JSON file into a render context. A named-but-unreadable or malformed file is a
/// hard error rather than a silent empty context: the whole point of passing it is that some
/// attribute cannot be written without it, and falling back quietly produces a page that builds
/// clean and is broken in the browser.
fn load_globals(path: &Path) -> Value {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to read globals file {}: {}", path.display(), e);
            std::process::exit(1);
        }
    };
    match serde_json::from_str::<Value>(&raw) {
        Ok(Value::Object(map)) => Value::Object(map),
        Ok(_) => {
            eprintln!("Globals file {} must contain a JSON object", path.display());
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Failed to parse globals file {}: {}", path.display(), e);
            std::process::exit(1);
        }
    }
}

fn cmd_build(
    input: &Path,
    output: &Path,
    globals: Option<&Path>,
    middleware: &[PathBuf],
    verbose: bool,
) {
    // Never walk into the output directory - see `is_excluded_dir`.
    let files = collect_slurp_files_excluding(input, Some(output));

    if files.is_empty() {
        eprintln!("No .slurp files found in {}", input.display());
        return;
    }

    if verbose {
        println!("Building {} file(s)...", files.len());
    }

    // Build file registry: "@sections/Header" → source text
    let mut registry_map: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for file in &files {
        if let Ok(rel) = file.strip_prefix(input) {
            let key = format!(
                "@{}",
                rel.with_extension("").to_string_lossy().replace('\\', "/")
            );
            if let Ok(source) = std::fs::read_to_string(file) {
                registry_map.insert(key, source);
            }
        }
    }
    let file_registry = std::sync::Arc::new(registry_map);

    let mut total_errors = 0usize;
    // Counted separately from `files.len()` so the summary line cannot claim to have compiled a
    // middleware file it deliberately skipped.
    let mut middleware_checked = 0usize;
    // Components, layouts and anything else outside `pages/`: compiled and checked, never written.
    let mut non_page_checked = 0usize;
    // Empty context for build-time compilation - real data is injected by the runtime. `--globals`
    // seeds the values that CANNOT wait for the runtime (see the flag's docs).
    let context = match globals {
        Some(p) => load_globals(p),
        None => Value::Object(serde_json::Map::new()),
    };

    for file in &files {
        let source = match std::fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error reading {}: {}", file.display(), e);
                total_errors += 1;
                continue;
            }
        };

        let file_str = file.to_string_lossy();

        // Middleware is checked, then skipped. Rendering it would be meaningless - it runs per
        // request in the host and its whole output is `{redirect}` / `{next}` signals, which render
        // to nothing - and writing a `.html` for it would put a file in `dist/` that no route ever
        // serves. Checking it here is the point: its scope rules are inverted, so the ordinary page
        // walk reports every correct middleware file as a violation and lets a broken one through.
        if is_middleware_file(file, input, middleware) {
            middleware_checked += 1;
            match parse(&source, file_str.as_ref()) {
                Err(lex_err) => {
                    eprintln!("  error: {lex_err}");
                    total_errors += 1;
                }
                Ok((doc, mut diag)) => {
                    if let Err(sec_err) = check_security(&doc, file_str.as_ref(), true) {
                        diag.push(sec_err);
                    }
                    print_diagnostics(&diag, file, verbose);
                    total_errors += diag.errors.len();
                    if verbose && !diag.has_errors() {
                        println!("  {} (middleware, not emitted)", file.display());
                    }
                }
            }
            continue;
        }

        let (html, diag) = compile_with_registry(
            &source,
            file_str.as_ref(),
            &context,
            BuildMode::Production,
            std::sync::Arc::clone(&file_registry),
        );

        print_diagnostics(&diag, file, verbose);
        total_errors += diag.errors.len();

        // Compile everything, emit only pages.
        //
        // A component or layout rendered on its own is not a document: it is a fragment, and a
        // layout comes out carrying literal `<slot>` elements because nothing filled them. The
        // example theme emitted ten such files against six real pages. Nothing can serve them
        // either - `pages/` is the routing convention, not a suggestion (`serve_page` resolves a
        // bare path to `pages/{name}.html`), so a file outside it is unreachable by construction.
        //
        // They are still COMPILED, and that is deliberate rather than wasteful: a syntax error, a
        // security violation or an unknown filter in a component that no page happens to import
        // yet is caught here, and would not be if these were skipped the way middleware is.
        if let Some(kind) = non_page_kind(file, input) {
            non_page_checked += 1;
            if verbose && !diag.has_errors() {
                println!("  {} ({kind}, checked only)", file.display());
            }
            continue;
        }

        if !diag.has_errors() {
            let rel = file.strip_prefix(input).unwrap_or(file);
            let out_path = output.join(rel).with_extension("html");
            if let Some(parent) = out_path.parent()
                && let Err(e) = std::fs::create_dir_all(parent)
            {
                eprintln!(
                    "Failed to create output directory {}: {}",
                    parent.display(),
                    e
                );
                continue;
            }
            let output_bytes =
                minify_html::minify(html.as_bytes(), &minify_html::Cfg::spec_compliant());
            if let Err(e) = std::fs::write(&out_path, &output_bytes) {
                eprintln!("Failed to write {}: {}", out_path.display(), e);
                total_errors += 1;
            } else if verbose {
                println!("  {} → {}", file.display(), out_path.display());
            }
        }
    }

    if total_errors > 0 {
        eprintln!("\nBuild failed with {total_errors} error(s).");
        process::exit(1);
    }

    let pages = files.len() - middleware_checked - non_page_checked;
    let mut summary = format!("Build complete. {pages} page(s) emitted");
    if non_page_checked > 0 {
        summary.push_str(&format!(
            ", {non_page_checked} component/layout file(s) checked"
        ));
    }
    if middleware_checked > 0 {
        summary.push_str(&format!(
            ", {middleware_checked} middleware file(s) checked"
        ));
    }
    println!("{summary}.");

    // A build that emits nothing is almost always a misplaced tree rather than an empty project,
    // and saying "Build complete. 0 page(s) emitted" without explaining why sends the reader
    // looking for a bug in their templates.
    if pages == 0 && !files.is_empty() {
        eprintln!(
            "\nwarning: no pages were emitted. Pages live in a `pages/` directory at the root of\n\
             the input tree; {} .slurp file(s) were found and checked, but none is under `pages/`.",
            files.len()
        );
    }

    copy_static_assets(input, output, globals);
}

fn copy_static_assets(input: &Path, output: &Path, globals: Option<&Path>) {
    // The output directory is excluded here too, and for a sharper reason than in the compile
    // walk: this copies every non-`.slurp` file it finds, so walking the output would copy the
    // HTML just emitted back into a nested copy of itself, growing the tree on every build.
    let skip = resolve_for_compare(output);
    // The globals file is BUILD input, not a site asset. It sat inside the input tree in the
    // obvious layout, so the copy shipped it to production: at best a stray file, at worst
    // whatever the author put in it, published at a guessable path.
    let globals = globals.and_then(resolve_for_compare);
    copy_static_recursive(input, input, output, skip.as_deref(), globals.as_deref());
}

fn copy_static_recursive(
    root: &Path,
    dir: &Path,
    output: &Path,
    skip: Option<&Path>,
    globals: Option<&Path>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_symlink() {
            continue;
        }
        if path.is_dir() {
            if !is_excluded_dir(&path, skip) {
                copy_static_recursive(root, &path, output, skip, globals);
            }
        } else if globals.is_some_and(|g| resolve_for_compare(&path).as_deref() == Some(g)) {
            continue;
        } else if path.extension().and_then(|e| e.to_str()) != Some("slurp")
            && let Ok(rel) = path.strip_prefix(root)
        {
            let dst = output.join(rel);
            if let Some(parent) = dst.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::copy(&path, &dst);
        }
    }
}

/// Whether `file` falls under one of the `--middleware` directories, each given relative to `root`.
///
/// Comparison is on the resolved paths so that `--middleware ./middleware/` and `--middleware
/// middleware` mean the same thing, and so a symlinked or `..`-containing argument cannot select a
/// directory outside the tree being scanned. An argument naming a directory that does not exist
/// matches nothing, which is the safe direction: the file is then checked under the stricter
/// non-middleware rules.
///
/// The two sides are compared at the SAME resolution level, never one canonicalized against one
/// merely absolute. `canonicalize` is fallible and its failures are situational - on Windows it
/// returns a `\\?\` verbatim path and can transiently fail with a sharing violation - so mixing the
/// two forms yields a comparison that depends on which call happened to succeed. That is a
/// selector deciding which of two INVERTED rule sets a file is checked under, so a comparison whose
/// answer varies run to run is not acceptable even though its failure mode is the safe direction.
/// The two directory names whose contents are INCLUDED by other templates rather than routed to.
///
/// `@components/...` and `@layouts/...` are the registry paths the language itself uses, so these
/// are conventions of the framework rather than a guess about someone's layout.
const NON_PAGE_DIRS: [&str; 2] = ["components", "layouts"];

/// `Some(kind)` when `file` is a template that must be checked but never emitted as a page.
///
/// This is a BLOCKLIST, not an allowlist of `pages/`, and the difference is the whole point. A
/// component or layout rendered standalone is a fragment - a layout comes out carrying literal
/// `<slot>` elements - so those must never be written. Everything else might legitimately be a
/// page: requiring `pages/` would mean the smallest possible project, a lone `index.slurp`, built
/// to nothing, which is exactly what the quickstart is.
///
/// Matched on the first path COMPONENT, so `components-archive/` is not caught by it.
fn non_page_kind(file: &Path, root: &Path) -> Option<&'static str> {
    let first = file.strip_prefix(root).ok()?.components().next()?;
    NON_PAGE_DIRS
        .into_iter()
        .find(|d| first.as_os_str() == *d)
        .map(|d| {
            if d == "components" {
                "component"
            } else {
                "layout"
            }
        })
}

fn is_middleware_file(file: &Path, root: &Path, dirs: &[PathBuf]) -> bool {
    if dirs.is_empty() {
        return false;
    }
    let file_abs = std::path::absolute(file).ok();
    let file_canon = std::fs::canonicalize(file).ok();

    dirs.iter().any(|d| {
        let candidate = root.join(d);
        // Canonical against canonical when BOTH sides resolve; absolute against absolute
        // otherwise. Never one of each.
        match (&file_canon, std::fs::canonicalize(&candidate).ok()) {
            (Some(f), Some(dir)) => f.starts_with(dir),
            _ => match (&file_abs, std::path::absolute(&candidate).ok()) {
                // The directory must exist to select anything: a typo'd `--middleware` argument
                // silently selecting nothing is better than it silently selecting a tree.
                (Some(f), Some(dir)) => candidate.is_dir() && f.starts_with(dir),
                _ => false,
            },
        }
    })
}

fn cmd_validate(dir: &Path, middleware: &[PathBuf], show_warnings: bool) {
    let files = collect_slurp_files(dir);

    if files.is_empty() {
        println!("No .slurp files found in {}", dir.display());
        return;
    }

    let mut total_errors = 0usize;
    let mut total_warnings = 0usize;

    for file in &files {
        let source = match std::fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error reading {}: {}", file.display(), e);
                total_errors += 1;
                continue;
            }
        };

        let file_str = file.to_string_lossy();

        match parse(&source, file_str.as_ref()) {
            Err(lex_err) => {
                eprintln!("  error: {lex_err}");
                total_errors += 1;
            }
            Ok((doc, diag)) => {
                for err in &diag.errors {
                    eprintln!(
                        "  error[{:?}]: {} ({}:{}:{})",
                        err.code,
                        err.message,
                        file.display(),
                        err.line,
                        err.column
                    );
                }
                // Validation must enforce what a BUILD enforces, or a theme author gets a clean
                // `slurp validate` and then a security error at publish time with no way to have
                // seen it locally. Middleware is a compile option rather than a path convention, so
                // the caller states which files are middleware with `--middleware`; anything not
                // named is checked under the ordinary page rules.
                let as_middleware = is_middleware_file(file, dir, middleware);
                if let Err(sec_err) = check_security(&doc, file_str.as_ref(), as_middleware) {
                    eprintln!(
                        "  error[{:?}]: {} ({}:{}:{})",
                        sec_err.code,
                        sec_err.message,
                        file.display(),
                        sec_err.line,
                        sec_err.column
                    );
                    total_errors += 1;
                }
                if show_warnings {
                    for warn in &diag.warnings {
                        eprintln!(
                            "  warning[{:?}]: {} ({}:{}:{})",
                            warn.code,
                            warn.message,
                            file.display(),
                            warn.line,
                            warn.column
                        );
                    }
                }
                total_errors += diag.errors.len();
                total_warnings += diag.warnings.len();
            }
        }
    }

    let status_label = if total_errors == 0 { "ok" } else { "fail" };
    println!(
        "Validation {}: {} file(s), {} error(s), {} warning(s).",
        status_label,
        files.len(),
        total_errors,
        total_warnings,
    );

    if total_errors > 0 {
        process::exit(1);
    }
}

fn cmd_dev(bind: &str, port: u16, fixtures: Option<&Path>) {
    // The dev server (file watcher, WebSocket HMR, SSR) lives in the standalone
    // `slurp-dev` binary from the `slurp-dev-server` crate. The compiler cannot
    // depend on that crate directly without creating a dependency cycle, so
    // `slurp dev` locates and launches its binary instead.
    let exe_name = if cfg!(windows) {
        "slurp-dev.exe"
    } else {
        "slurp-dev"
    };

    // Prefer a `slurp-dev` sitting next to this `slurp` binary (the usual cargo
    // build layout); otherwise fall back to whatever is on PATH.
    let program = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|dir| dir.join(exe_name)))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from(exe_name));

    if bind != "127.0.0.1" {
        eprintln!("note: slurp-dev binds 127.0.0.1; --bind {bind} is ignored.");
    }

    let mut command = process::Command::new(&program);
    command.arg("--port").arg(port.to_string());
    if let Some(path) = fixtures {
        command.arg("--fixtures").arg(path);
    }

    match command.status() {
        Ok(status) => process::exit(status.code().unwrap_or(1)),
        Err(_) => {
            eprintln!(
                "Could not launch the dev server binary `{exe_name}`.\n\
                 Build it with `cargo build -p slurp-dev-server`, then re-run `slurp dev`\n\
                 (or run `{exe_name} --help` directly for the full set of options)."
            );
            process::exit(127);
        }
    }
}

/// Resolve a path to a stable form so two spellings of the same directory compare equal.
///
/// Canonicalise when the path exists, which is the case that matters: the output directory is
/// only walkable at all once a previous run created it. Fall back to an absolute path for a
/// first run, where the output directory does not exist yet and therefore cannot be entered.
fn resolve_for_compare(p: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(p)
        .ok()
        .or_else(|| std::path::absolute(p).ok())
}

/// True when `dir` is a directory neither walk should descend into.
///
/// `skip` is the RESOLVED output directory. Without it, `slurp build --output out` walks its own
/// output: run one, and the second run finds `out/`, compiles the pages it already emitted into
/// `out/out/`, and every run after that adds a level - reproducible as `out/out/out/...`. The
/// hardcoded names below only ever hid this for the DEFAULT `dist`, so the bug appeared the
/// moment anyone passed `--output`. Compare resolved paths rather than names, because the
/// output directory can be spelled `./out`, `out/`, or an absolute path, and can legitimately
/// be called anything.
fn is_excluded_dir(path: &Path, skip: Option<&Path>) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name.starts_with('.') || name == "node_modules" || name == "dist" || name == "target" {
        return true;
    }
    match (skip, resolve_for_compare(path)) {
        (Some(skip), Some(resolved)) => resolved == skip,
        _ => false,
    }
}

fn collect_slurp_files(dir: &Path) -> Vec<PathBuf> {
    collect_slurp_files_excluding(dir, None)
}

fn collect_slurp_files_excluding(dir: &Path, output: Option<&Path>) -> Vec<PathBuf> {
    let skip = output.and_then(resolve_for_compare);
    let mut files = Vec::new();
    collect_recursive(dir, &mut files, skip.as_deref());
    files.sort();
    files
}

fn collect_recursive(dir: &Path, out: &mut Vec<PathBuf>, skip: Option<&Path>) {
    let read_dir = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return,
    };

    for entry in read_dir.flatten() {
        let path = entry.path();
        // Skip symlinks to avoid following links that escape the project directory.
        if path.is_symlink() {
            continue;
        }
        if path.is_dir() {
            if !is_excluded_dir(&path, skip) {
                collect_recursive(&path, out, skip);
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("slurp") {
            out.push(path);
        }
    }
}

fn print_diagnostics(diag: &ErrorAccumulator, _file: &Path, verbose: bool) {
    for err in &diag.errors {
        eprintln!(
            "  error[{:?}]: {} ({}:{})",
            err.code, err.message, err.line, err.column
        );
    }
    if verbose {
        for warn in &diag.warnings {
            eprintln!(
                "  warning[{:?}]: {} ({}:{})",
                warn.code, warn.message, warn.line, warn.column
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory that removes itself, so these tests add no dev-dependency.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let n = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let p =
                std::env::temp_dir().join(format!("slurp-cli-{tag}-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&p).expect("temp dir");
            TempDir(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(path, body).expect("write");
    }

    /// REGRESSION: `slurp build --output out` compiled its own output on the next run.
    ///
    /// The walk skipped `node_modules`, `dist` and `target` BY NAME, so the default output
    /// directory was excluded by coincidence and any other `--output` was not. A second build
    /// therefore found the HTML-adjacent `.slurp` copies under `out/`, emitted them to
    /// `out/out/`, and each further run added a level.
    #[test]
    fn the_build_walk_skips_its_own_output_directory_whatever_it_is_named() {
        let tmp = TempDir::new("outwalk");
        let input = tmp.path();
        let output = input.join("out");

        write(&input.join("pages/index.slurp"), "<p>hi</p>");
        // Stand in for a previous run's output: a `.slurp` file already inside `out/`.
        write(&output.join("pages/index.slurp"), "<p>stale</p>");

        let found = collect_slurp_files_excluding(input, Some(&output));
        assert_eq!(
            found.len(),
            1,
            "expected only the source page, got {found:?}"
        );
        assert!(
            found[0].starts_with(input.join("pages")),
            "the output directory was walked: {found:?}"
        );
    }

    /// The exclusion is by RESOLVED PATH, not by the string that was typed. `--output ./out`
    /// and `--output out` name one directory, and a name check on `"./out"` matches neither.
    #[test]
    fn the_output_directory_is_matched_however_it_is_spelled() {
        let tmp = TempDir::new("outspell");
        let input = tmp.path();
        let output = input.join("build-html");
        write(&input.join("a.slurp"), "<p>a</p>");
        write(&output.join("a.slurp"), "<p>stale</p>");

        for spelling in [
            output.clone(),
            input.join("./build-html"),
            input.join("sub/../build-html"),
        ] {
            let found = collect_slurp_files_excluding(input, Some(&spelling));
            assert_eq!(
                found.len(),
                1,
                "spelling {spelling:?} did not exclude the output directory: {found:?}"
            );
        }
    }

    /// Without an output directory to exclude, nothing else changes: the hardcoded skips
    /// (hidden dirs, `node_modules`, `dist`, `target`) still apply, and everything else is
    /// still collected. This is what stops the fix narrowing the walk.
    #[test]
    fn the_walk_still_collects_everything_else() {
        let tmp = TempDir::new("outkeep");
        let input = tmp.path();
        write(&input.join("a.slurp"), "<p>a</p>");
        write(&input.join("deep/nested/b.slurp"), "<p>b</p>");
        write(&input.join("node_modules/c.slurp"), "<p>c</p>");
        write(&input.join("dist/d.slurp"), "<p>d</p>");
        write(&input.join("target/e.slurp"), "<p>e</p>");
        write(&input.join(".hidden/f.slurp"), "<p>f</p>");

        let found = collect_slurp_files_excluding(input, Some(&input.join("out")));
        let names: Vec<String> = found
            .iter()
            .filter_map(|p| p.file_name()?.to_str().map(str::to_owned))
            .collect();
        assert_eq!(names, vec!["a.slurp", "b.slurp"], "got {found:?}");
    }

    /// The static-asset copy has the same walk and the same bug: it copies every
    /// non-`.slurp` file, so walking the output would copy the emitted HTML into a nested
    /// copy of itself on every build.
    #[test]
    fn the_static_asset_copy_skips_the_output_directory_too() {
        let tmp = TempDir::new("outstatic");
        let input = tmp.path();
        let output = input.join("out");

        write(&input.join("css/site.css"), "body{}");
        // A previous run's emitted page, sitting in the output directory.
        write(&output.join("index.html"), "<p>emitted</p>");

        copy_static_assets(input, &output, None);

        assert!(
            output.join("css/site.css").is_file(),
            "a real static asset was not copied"
        );
        assert!(
            !output.join("out").exists(),
            "the output directory was copied into itself"
        );
    }

    /// The embedded template and the committed example must not drift.
    ///
    /// `slurp gitignore` serves the const, so a reader who edits the example file and expects the
    /// command to follow would be wrong, silently. Skipped when the example is absent, which is
    /// the case in a published crate: `examples/` is not part of the package.
    #[test]
    fn the_embedded_theme_gitignore_matches_the_committed_example() {
        let example = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("examples")
            .join("basic-theme")
            .join(".gitignore");
        let Ok(on_disk) = std::fs::read_to_string(&example) else {
            return;
        };
        // Compared with line endings normalised: `.gitattributes` pins the working tree to LF,
        // but a checkout that predates it, or an editor configured otherwise, would otherwise
        // fail this test for a reason that has nothing to do with drift.
        let normalise = |s: &str| s.replace("\r\n", "\n");
        assert_eq!(
            normalise(&on_disk),
            normalise(THEME_GITIGNORE),
            "examples/basic-theme/.gitignore has drifted from THEME_GITIGNORE in main.rs. \
             The const is what `slurp gitignore` writes; copy it over the example."
        );
    }

    /// Refusing to clobber is the behaviour, so it is the thing worth pinning: the likely caller
    /// is in a directory that already has a `.gitignore` full of their own rules.
    #[test]
    fn writing_a_gitignore_never_silently_replaces_one() {
        let tmp = TempDir::new("gitignore");
        let dir = tmp.path();
        let target = dir.join(".gitignore");

        write(
            &target,
            "my-own-rule/
",
        );
        // `cmd_gitignore` exits the process on refusal, so the refusal path itself is covered by
        // the CLI test above rather than here. What this pins is the APPEND merge, which is the
        // branch that can lose data.
        cmd_gitignore(dir, true, false, false);

        let merged = std::fs::read_to_string(&target).expect("read back");
        assert!(
            merged.starts_with("my-own-rule/"),
            "the existing rules were lost"
        );
        assert!(
            merged.contains("Added by `slurp gitignore`"),
            "no provenance header"
        );
        assert!(merged.contains("dist/"), "the template was not appended");
    }

    /// Only `pages/` is emitted, and everything else is still CHECKED.
    ///
    /// A layout rendered standalone is a fragment carrying literal `<slot>` elements, and nothing
    /// can route to it; the example theme used to write ten such files beside six real pages. The
    /// checking half matters just as much: an error in a component that no page imports yet must
    /// still fail the build, which it would not if these were skipped rather than compiled.
    #[test]
    fn only_pages_are_emitted_and_everything_else_is_still_checked() {
        let tmp = TempDir::new("pagesonly");
        let input = tmp.path();
        let output = input.join("dist");

        write(&input.join("pages/index.slurp"), "<p>hi</p>");
        write(&input.join("layouts/base.slurp"), "<html><slot /></html>");
        write(&input.join("components/Card.slurp"), "<div>card</div>");

        assert!(non_page_kind(&input.join("pages/index.slurp"), input).is_none());
        assert_eq!(
            non_page_kind(&input.join("layouts/base.slurp"), input),
            Some("layout")
        );
        assert_eq!(
            non_page_kind(&input.join("components/Card.slurp"), input),
            Some("component")
        );
        // A lone file at the root IS a page. Requiring `pages/` would build the smallest
        // possible project - the one the quickstart shows - to nothing.
        write(&input.join("index.slurp"), "<p>root</p>");
        assert!(non_page_kind(&input.join("index.slurp"), input).is_none());
        // A directory whose name merely starts with a blocked one is not blocked.
        write(&input.join("components-archive/old.slurp"), "<p>old</p>");
        assert!(non_page_kind(&input.join("components-archive/old.slurp"), input).is_none());

        cmd_build(input, &output, None, &[], false);

        assert!(
            output.join("pages/index.html").is_file(),
            "the page was not emitted"
        );
        assert!(
            !output.join("layouts/base.html").exists(),
            "a layout was emitted as a standalone page"
        );
        assert!(
            !output.join("components/Card.html").exists(),
            "a component was emitted as a standalone page"
        );
    }

    /// The `--globals` file is BUILD input. It sat inside the input tree in the obvious layout, so
    /// the static-asset copy published it at a guessable path alongside the site.
    #[test]
    fn the_globals_file_is_not_copied_into_the_output() {
        let tmp = TempDir::new("globalscopy");
        let input = tmp.path();
        let output = input.join("dist");
        let globals = input.join("globals.json");

        write(&input.join("pages/index.slurp"), "<p>${ site.title }</p>");
        write(&globals, r#"{"site":{"title":"T"}}"#);
        write(&input.join("css/site.css"), "body{}");

        cmd_build(input, &output, Some(&globals), &[], false);

        assert!(
            !output.join("globals.json").exists(),
            "the globals file was published with the site"
        );
        assert!(
            output.join("css/site.css").is_file(),
            "a real static asset stopped being copied"
        );
    }

    /// The `--middleware` selector, which decides which of two INVERTED rule sets a file is
    /// checked under. Getting it wrong in either direction is silent: too broad and a page may
    /// read `request.*`, too narrow and a correct middleware file fails to validate.
    #[test]
    fn middleware_selection_matches_only_files_under_a_named_directory() {
        let tmp = TempDir::new("mwsel");
        let root = tmp.path();
        write(&root.join("middleware/auth.slurp"), "x");
        write(&root.join("middleware/nested/deep.slurp"), "x");
        write(&root.join("pages/index.slurp"), "x");
        // A sibling whose name merely starts with the selected one. A plain string prefix test
        // would match this; a path component test does not.
        write(&root.join("middleware-helpers/util.slurp"), "x");

        let dirs = vec![PathBuf::from("middleware")];

        assert!(is_middleware_file(
            &root.join("middleware/auth.slurp"),
            root,
            &dirs
        ));
        assert!(
            is_middleware_file(&root.join("middleware/nested/deep.slurp"), root, &dirs),
            "selection must reach nested files"
        );
        assert!(!is_middleware_file(
            &root.join("pages/index.slurp"),
            root,
            &dirs
        ));
        assert!(
            !is_middleware_file(&root.join("middleware-helpers/util.slurp"), root, &dirs),
            "a sibling sharing a name prefix was selected"
        );

        // No flag means no middleware, which is what keeps the default strict.
        assert!(!is_middleware_file(
            &root.join("middleware/auth.slurp"),
            root,
            &[]
        ));

        // A directory that does not exist selects nothing rather than everything.
        assert!(!is_middleware_file(
            &root.join("middleware/auth.slurp"),
            root,
            &[PathBuf::from("does-not-exist")]
        ));

        // Spelling variants of the same directory are the same directory.
        for spelling in ["./middleware", "middleware/", "pages/../middleware"] {
            assert!(
                is_middleware_file(
                    &root.join("middleware/auth.slurp"),
                    root,
                    &[PathBuf::from(spelling)]
                ),
                "spelling {spelling} did not resolve to the same directory"
            );
        }
    }
}
