//! Persistent local memory core for SC Node.
//!
//! This crate defines backend-agnostic types and traits for storing and
//! retrieving embedded text ("memories"), plus a fully working, pure-Rust
//! persistent reference backend ([`ReferenceMemoryStore`]).
//!
//! Nothing here is coupled to any specific vector-search backend:
//! `sc-agent-core` and other consumers should depend on the [`MemoryStore`]
//! and [`EmbeddingProvider`] traits, not on `ReferenceMemoryStore` or the
//! optional `turbovec` backend directly.

pub mod error;
pub mod fake_embedding;
pub mod rag;
pub mod reference;
pub mod traits;
pub mod types;

#[cfg(feature = "turbovec")]
pub mod turbovec_backend;

pub use error::MemoryError;
pub use fake_embedding::FakeEmbeddingProvider;
pub use rag::{FakeAnswer, chunk_text, fake_answer};
pub use reference::ReferenceMemoryStore;
pub use traits::{EmbeddingProvider, MemoryStore, needs_reindex};
pub use types::{
    MemoryEntry, MemoryId, MemoryMetadata, MemoryQuery, MemorySearchResult, cosine_similarity,
};

#[cfg(feature = "turbovec")]
pub use turbovec_backend::TurboVecMemoryStore;
