use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompileError {
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub message: String,
    pub severity: Severity,
    pub code: ErrorCode,
}

impl CompileError {
    #[must_use]
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

    pub fn lex(
        file: impl Into<String>,
        line: usize,
        column: usize,
        message: impl Into<String>,
    ) -> Self {
        Self::new(
            file,
            line,
            column,
            message,
            Severity::Error,
            ErrorCode::UnexpectedToken,
        )
    }

    pub fn parse(
        file: impl Into<String>,
        line: usize,
        column: usize,
        message: impl Into<String>,
        code: ErrorCode,
    ) -> Self {
        Self::new(file, line, column, message, Severity::Error, code)
    }

    pub fn render(
        file: impl Into<String>,
        line: usize,
        column: usize,
        message: impl Into<String>,
        code: ErrorCode,
    ) -> Self {
        Self::new(file, line, column, message, Severity::Error, code)
    }

    pub fn warn(
        file: impl Into<String>,
        line: usize,
        column: usize,
        message: impl Into<String>,
        code: ErrorCode,
    ) -> Self {
        Self::new(file, line, column, message, Severity::Warning, code)
    }
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{:?}] {} at {}:{}:{} ({})",
            self.code, self.message, self.file, self.line, self.column, self.severity
        )
    }
}

impl std::error::Error for CompileError {}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Severity::Error => write!(f, "error"),
            Severity::Warning => write!(f, "warning"),
            Severity::Info => write!(f, "info"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ErrorCode {
    UnexpectedToken,
    UnclosedBlock,
    InvalidFrontmatter,
    CircularImport,
    MissingFile,
    MissingRequiredProp,
    UnknownProp,
    TypeMismatch,
    SecretEnvInTemplate,
    MiddlewareScopeViolation,
    IterationLimitExceeded,
    MissingImageSrc,
    LayoutDepthExceeded,
    UnknownComponent,
    UnknownSlot,
    UnknownFilter,
    InvalidFilterArgs,
    RedirectOutsideMiddleware,
    InfiniteLoop,
    /// A `${ ... }` inside a `<script>` body that does not declare how it should
    /// be escaped. See `renderer::Renderer::render_script_expression`.
    UnsafeScriptInterpolation,
    /// A JavaScript template literal written in an HTML attribute value, whose
    /// `${ }` slot slurp will interpolate itself. See
    /// `renderer::js_slot::template_literal_in_attr`.
    JsTemplateLiteralInAttribute,
}

pub type CompileResult<T> = Result<T, CompileError>;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ErrorAccumulator {
    pub errors: Vec<CompileError>,
    pub warnings: Vec<CompileError>,
}

impl ErrorAccumulator {
    /// SECURITY (memory): upper bound on total accumulated diagnostics (errors + warnings).
    ///
    /// A pathological input can raise a diagnostic per token before anything bails, so without a
    /// ceiling the accumulator is an unbounded-memory vector in its own right. Past this, `push`
    /// is a no-op: the input is already unparseable and further diagnostics say nothing new.
    pub const MAX_DIAGNOSTICS: usize = 1000;

    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, error: CompileError) {
        if self.errors.len() + self.warnings.len() >= Self::MAX_DIAGNOSTICS {
            return;
        }
        match error.severity {
            Severity::Error => self.errors.push(error),
            Severity::Warning | Severity::Info => self.warnings.push(error),
        }
    }

    #[must_use]
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    pub fn all(&self) -> impl Iterator<Item = &CompileError> {
        self.errors.iter().chain(self.warnings.iter())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_vec_is_bounded() {
        // `push` must stay a no-op past MAX_DIAGNOSTICS however many arrive.
        let mut acc = ErrorAccumulator::new();
        for i in 0..(ErrorAccumulator::MAX_DIAGNOSTICS * 3) {
            acc.push(CompileError::parse(
                "f",
                i,
                0,
                "boom",
                ErrorCode::UnexpectedToken,
            ));
        }
        assert_eq!(acc.errors.len(), ErrorAccumulator::MAX_DIAGNOSTICS);
        assert!(acc.errors.len() + acc.warnings.len() <= ErrorAccumulator::MAX_DIAGNOSTICS);
    }
}
