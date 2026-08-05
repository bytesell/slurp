use std::path::{Path, PathBuf};

use crate::errors::{DevResult, DevServerError, ErrorCode, Severity, SlurpError};

/// Syntax-check a changed `.slurp` file and return whatever the real compiler complained about.
///
/// This deliberately PARSES rather than renders. A full render needs a context and a component
/// registry that only a build has (`slurp build`, which the source watcher already runs, and
/// `ssr_render` for a live request), so re-rendering here would either duplicate that or lie about
/// the result. Parsing is the part that is meaningful on every save: it is what turns a typo into
/// the HMR error overlay instead of a silently stale page.
pub async fn check_page(path: &Path) -> DevResult<Vec<SlurpError>> {
    let source_text = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| DevServerError::Io {
            path: path.display().to_string(),
            source: e,
        })?;

    let file = path.display().to_string();
    match slurp_compiler::parse(&source_text, &file) {
        // Lexing failed outright: one fatal error and nothing else to say.
        Err(fatal) => Ok(vec![to_slurp_error(&fatal)]),
        Ok((_doc, diagnostics)) => Ok(diagnostics
            .errors
            .iter()
            .chain(diagnostics.warnings.iter())
            .map(to_slurp_error)
            .collect()),
    }
}

/// Map a compiler diagnostic onto the dev server's wire type. The two `ErrorCode` enums overlap
/// only partly; anything the wire type cannot name is reported as `CompilerFailed` rather than
/// being silently renamed to a code that means something else.
fn to_slurp_error(err: &slurp_compiler::CompileError) -> SlurpError {
    use slurp_compiler::ErrorCode as C;
    let code = match err.code {
        C::UnexpectedToken => ErrorCode::UnexpectedToken,
        C::UnclosedBlock => ErrorCode::UnclosedBlock,
        C::InvalidFrontmatter => ErrorCode::InvalidFrontmatter,
        C::CircularImport => ErrorCode::CircularImport,
        C::MissingFile => ErrorCode::MissingFile,
        C::SecretEnvInTemplate | C::MiddlewareScopeViolation => ErrorCode::Security,
        _ => ErrorCode::CompilerFailed,
    };
    let severity = match err.severity {
        slurp_compiler::Severity::Error => Severity::Error,
        slurp_compiler::Severity::Warning => Severity::Warning,
        slurp_compiler::Severity::Info => Severity::Info,
    };
    SlurpError::new(
        err.file.clone(),
        err.line,
        err.column,
        err.message.clone(),
        severity,
        code,
    )
}

pub fn extract_imports(source_file: &Path, text: &str) -> DevResult<Vec<PathBuf>> {
    let mut imports = Vec::new();
    let parent = source_file.parent().unwrap_or(Path::new("."));

    let mut in_frontmatter = false;
    let mut delimiter_count = 0usize;

    for line in text.lines() {
        let trimmed = line.trim();

        if trimmed == "---" {
            delimiter_count += 1;
            in_frontmatter = delimiter_count == 1;
            if delimiter_count == 2 {
                break;
            }
            continue;
        }

        if !in_frontmatter {
            continue;
        }

        if let Some(path_str) = parse_using_path(trimmed) {
            // Skip HTTP(S) imports - only resolve filesystem paths.
            if path_str.starts_with("http://") || path_str.starts_with("https://") {
                continue;
            }

            let resolved = resolve_import(parent, path_str);
            imports.push(resolved);
        }
    }

    Ok(imports)
}

fn parse_using_path(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("using")?;
    let rest = rest.trim_start();

    let rest = if rest.starts_with("type") {
        let from_idx = rest.find("from")?;
        rest[from_idx + 4..].trim_start()
    } else {
        rest
    };

    extract_quoted(rest)
}

fn extract_quoted(s: &str) -> Option<&str> {
    let s = s.trim_start();
    let quote_char = s.chars().next()?;
    if quote_char != '"' && quote_char != '\'' {
        return None;
    }
    let inner = &s[1..];
    let end = inner.find(quote_char)?;
    Some(&inner[..end])
}

// `@` aliases are not resolved here - they require a module registry.
fn resolve_import(parent: &Path, import_path: &str) -> PathBuf {
    let without_at = import_path.strip_prefix('@').unwrap_or(import_path);
    let path = Path::new(without_at);

    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        parent.join(path)
    };

    match resolved.extension().and_then(|e| e.to_str()) {
        Some("slurp") => resolved,
        _ => resolved.with_extension("slurp"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parses_simple_using() {
        let line = r#"using "@components/Navbar""#;
        assert_eq!(parse_using_path(line), Some("@components/Navbar"));
    }

    #[test]
    fn parses_type_using() {
        let line = r#"using type { Review } from "@components/ReviewCard""#;
        assert_eq!(parse_using_path(line), Some("@components/ReviewCard"));
    }

    #[test]
    fn ignores_non_using() {
        assert_eq!(parse_using_path("props"), None);
        assert_eq!(parse_using_path("--- "), None);
    }

    #[test]
    fn extract_imports_from_frontmatter() {
        let src = r#"---
using "@components/Navbar"
using type { Review } from "@components/ReviewCard"
props
  title: string
---
<div></div>"#;
        let file = PathBuf::from("/theme/pages/index.slurp");
        let imports = extract_imports(&file, src).expect("extract_imports failed");
        assert_eq!(imports.len(), 2);
    }

    #[test]
    fn no_frontmatter_returns_empty() {
        let src = "<div>Hello</div>";
        let file = PathBuf::from("/theme/pages/index.slurp");
        let imports = extract_imports(&file, src).expect("extract_imports failed");
        assert!(imports.is_empty());
    }
}
