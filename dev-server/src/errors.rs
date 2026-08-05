use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SlurpError {
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub message: String,
    pub severity: Severity,
    pub code: ErrorCode,
}

impl SlurpError {
    pub fn new(
        file: impl Into<String>,
        line: usize,
        column: usize,
        message: impl Into<String>,
        severity: Severity,
        code: ErrorCode,
    ) -> Self {
        Self {
            file: file.into(),
            line,
            column,
            message: message.into(),
            severity,
            code,
        }
    }

    pub fn runtime(message: impl Into<String>) -> Self {
        Self::new("", 0, 0, message, Severity::Error, ErrorCode::Runtime)
    }

    pub fn io(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(path, 0, 0, message, Severity::Error, ErrorCode::Io)
    }
}

impl std::fmt::Display for SlurpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{:?}] {} at {}:{}:{} ({})",
            self.code, self.message, self.file, self.line, self.column, self.severity
        )
    }
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Severity::Error => write!(f, "error"),
            Severity::Warning => write!(f, "warning"),
            Severity::Info => write!(f, "info"),
        }
    }
}

/// Wire form is LOWERCASE, matching `shared/interfaces.ts`
/// (`severity: 'error' | 'warning' | 'info'`) and the WASM producer in
/// `compiler/src/wasm/mod.rs`, which serialises via the `Display` impl above.
///
/// Without this rename the derive emits `"Error"`, which matches neither
/// comparison in `runtime/src/core/dev.ts`, so every HMR error card in the
/// browser renders in the info colour instead. TypeScript cannot catch that:
/// the message is parsed from JSON at runtime, so nothing but this attribute
/// keeps the two Rust producers agreeing on the wire form.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ErrorCode {
    UnexpectedToken,
    UnclosedBlock,
    InvalidFrontmatter,
    CircularImport,
    MissingFile,
    Io,
    Runtime,
    Security,
    CompilerFailed,
}

#[derive(Debug, Error)]
pub enum DevServerError {
    #[error("I/O error at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("File watcher error: {0}")]
    Watcher(#[from] notify::Error),

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Compilation error: {0}")]
    Compiler(String),

    #[error("Server bind error on port {port}: {source}")]
    Bind {
        port: u16,
        #[source]
        source: std::io::Error,
    },

    #[error("Theme directory not found: {0}")]
    ThemeDirNotFound(String),
}

impl DevServerError {
    pub fn to_slurp_error(&self) -> SlurpError {
        match self {
            DevServerError::Io { path, source } => SlurpError::io(path.clone(), source.to_string()),
            DevServerError::Watcher(e) => SlurpError::runtime(e.to_string()),
            DevServerError::Json(e) => SlurpError::runtime(e.to_string()),
            DevServerError::Compiler(msg) => SlurpError::runtime(msg.clone()),
            DevServerError::Bind { port, source } => {
                SlurpError::runtime(format!("Bind error on port {port}: {source}"))
            }
            DevServerError::ThemeDirNotFound(path) => SlurpError::io(path.clone(), "Not found"),
        }
    }
}

pub type DevResult<T> = Result<T, DevServerError>;
