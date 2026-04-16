//! Error types for the Claude Agent SDK.

use thiserror::Error;

/// Top-level error type for all SDK operations.
#[derive(Debug, Error)]
pub enum ClaudeSdkError {
    /// Failed to connect to the Claude Code CLI process.
    #[error("CLI connection error: {0}")]
    CliConnection(String),

    /// Claude Code CLI binary could not be found on disk.
    #[error("Claude Code not found{}", .cli_path.as_ref().map(|p| format!(": {p}")).unwrap_or_default())]
    CliNotFound {
        /// Path that was searched, if known.
        cli_path: Option<String>,
        /// Human-readable diagnostic message.
        message: String,
    },

    /// The CLI subprocess exited with a non-zero status.
    #[error("Process error: {message}{}{}",
        .exit_code.map(|c| format!(" (exit code: {c})")).unwrap_or_default(),
        .stderr.as_ref().map(|s| format!("\nError output: {s}")).unwrap_or_default(),
    )]
    Process {
        /// Diagnostic message.
        message: String,
        /// Exit code, if the process terminated normally.
        exit_code: Option<i32>,
        /// Captured stderr, if available.
        stderr: Option<String>,
    },

    /// JSON line received from the CLI failed to parse, even after buffering.
    #[error("Failed to decode JSON: {snippet}")]
    JsonDecode {
        /// First 100 bytes of the offending line.
        snippet: String,
        /// Underlying serde_json error message.
        source_message: String,
    },

    /// A CLI message did not match the expected schema.
    #[error("Message parse error: {message}")]
    MessageParse {
        /// Diagnostic message.
        message: String,
        /// Raw payload, if available.
        data: Option<serde_json::Value>,
    },

    /// I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization/deserialization failure (general).
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// A control-protocol request returned an error.
    #[error("Control request failed: {0}")]
    ControlRequest(String),

    /// A control-protocol request timed out.
    #[error("Control request timed out: {0}")]
    ControlTimeout(String),

    /// Generic invalid-argument error.
    #[error("Invalid argument: {0}")]
    InvalidArgument(String),

    /// File not found.
    #[error("File not found: {0}")]
    FileNotFound(String),

    /// Generic catch-all wrapping any other error.
    #[error("{0}")]
    Other(String),
}

impl ClaudeSdkError {
    pub(crate) fn cli_not_found(message: impl Into<String>, cli_path: Option<String>) -> Self {
        Self::CliNotFound { cli_path, message: message.into() }
    }

    pub(crate) fn cli_connection(message: impl Into<String>) -> Self {
        Self::CliConnection(message.into())
    }

    pub(crate) fn process(
        message: impl Into<String>,
        exit_code: Option<i32>,
        stderr: Option<String>,
    ) -> Self {
        Self::Process { message: message.into(), exit_code, stderr }
    }

    #[allow(dead_code)]
    pub(crate) fn json_decode(line: &str, err: &serde_json::Error) -> Self {
        let snippet: String = line.chars().take(100).collect();
        Self::JsonDecode { snippet: format!("{snippet}..."), source_message: err.to_string() }
    }

    pub(crate) fn message_parse(message: impl Into<String>, data: Option<serde_json::Value>) -> Self {
        Self::MessageParse { message: message.into(), data }
    }
}

/// Convenience `Result` alias.
pub type Result<T, E = ClaudeSdkError> = std::result::Result<T, E>;
