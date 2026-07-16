//! Error types for `sc-memory`.

use thiserror::Error;

use crate::types::MemoryId;

/// Errors returned by `MemoryStore` and `EmbeddingProvider` implementations.
#[derive(Debug, Error)]
pub enum MemoryError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    /// A stored file contains a line that could not be parsed as a valid
    /// log record. Carries the 1-based line number so the caller can
    /// locate the damage; never surfaces as a panic.
    #[error("corrupt memory store at line {line}: {reason}")]
    Corrupt { line: usize, reason: String },

    /// The store file's header declares a schema version this build does
    /// not know how to read.
    #[error("unsupported schema version {found} (this build supports {supported})")]
    UnsupportedSchemaVersion { found: u32, supported: u32 },

    #[error("memory entry not found: {0}")]
    NotFound(MemoryId),

    #[error("memory entry already exists: {0}")]
    AlreadyExists(MemoryId),

    #[error("embedding provider error: {0}")]
    Embedding(String),

    #[error("backend error: {0}")]
    Backend(String),
}
