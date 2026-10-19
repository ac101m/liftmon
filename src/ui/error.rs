//! Errors produced by the UI layer.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum UiError {
    /// Error from the underlying terminal (I/O, raw-mode setup, ...).
    #[error("terminal error: {0}")]
    Io(#[from] std::io::Error),

    /// The user pressed Ctrl+C to quit.
    #[error("interrupted")]
    Interrupted,
}
