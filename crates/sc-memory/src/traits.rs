//! Backend-agnostic traits: `MemoryStore` and `EmbeddingProvider`.
//!
//! Nothing in this module (or crate) depends on any specific vector
//! backend. `sc-agent-core` and other crates that want persistent memory
//! should depend on these traits, not on a concrete backend.

use async_trait::async_trait;

use crate::error::MemoryError;
use crate::types::{MemoryEntry, MemoryId, MemoryQuery, MemorySearchResult};

/// A persistent store of `MemoryEntry` records with metadata- and
/// workspace-scoped search.
#[async_trait]
pub trait MemoryStore: Send + Sync {
    /// Persist a new entry and return its id (`entry.id`).
    ///
    /// Returns [`MemoryError::AlreadyExists`] if an entry with this id is
    /// already present (this should not happen in practice since
    /// `MemoryId::new()` is randomly generated, but implementations must
    /// not silently overwrite on id collision).
    async fn add(&self, entry: MemoryEntry) -> Result<MemoryId, MemoryError>;

    /// Fetch an entry by id, or `None` if it does not exist.
    async fn get(&self, id: MemoryId) -> Result<Option<MemoryEntry>, MemoryError>;

    /// Replace an existing entry (matched by `entry.id`).
    ///
    /// Returns [`MemoryError::NotFound`] if no entry with this id exists.
    async fn update(&self, entry: MemoryEntry) -> Result<(), MemoryError>;

    /// Delete an entry by id. Returns `true` if it existed and was
    /// removed, `false` if there was nothing to delete (not an error).
    async fn delete(&self, id: MemoryId) -> Result<bool, MemoryError>;

    /// Search for entries matching `query`, ranked by similarity to
    /// `query.embedding`, highest score first.
    ///
    /// Implementations MUST only consider entries whose
    /// `MemoryMetadata::workspace` equals `query.workspace`: entries from
    /// any other workspace must never be returned.
    async fn search(&self, query: &MemoryQuery) -> Result<Vec<MemorySearchResult>, MemoryError>;
}

/// Produces embedding vectors for text.
///
/// Implementations report a `model_id` and `version` so callers (and
/// `MemoryStore` backends) can detect when stored entries were embedded by
/// a different model/version and need re-indexing.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Stable identifier for the embedding model, e.g. `"fake-hash-v1"`.
    fn model_id(&self) -> &str;

    /// Output vector dimensionality. Every vector returned by `embed` has
    /// exactly this length.
    fn dimension(&self) -> usize;

    /// Model/build version. Bump this whenever the same `model_id` starts
    /// producing vectors that are not comparable to previous ones.
    fn version(&self) -> u32;

    /// Embed `text` into a vector of length `self.dimension()`.
    async fn embed(&self, text: &str) -> Result<Vec<f32>, MemoryError>;
}

/// `true` if `entry` was embedded by a different model or version than
/// `provider` currently reports, meaning it should be re-embedded before
/// it can be meaningfully compared against fresh queries.
pub fn needs_reindex(entry: &MemoryEntry, provider: &dyn EmbeddingProvider) -> bool {
    entry.embedding_model != provider.model_id() || entry.embedding_version != provider.version()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_embedding::FakeEmbeddingProvider;
    use crate::types::MemoryMetadata;

    #[test]
    fn test_needs_reindex_detects_model_change() {
        let provider = FakeEmbeddingProvider::new(8);
        let entry = MemoryEntry::new(
            "hi",
            vec![0.0; 8],
            MemoryMetadata::new("ws"),
            "some-other-model",
            1,
        );
        assert!(needs_reindex(&entry, &provider));
    }

    #[test]
    fn test_needs_reindex_detects_version_change() {
        let provider = FakeEmbeddingProvider::new(8);
        let entry = MemoryEntry::new(
            "hi",
            vec![0.0; 8],
            MemoryMetadata::new("ws"),
            provider.model_id(),
            provider.version() + 1,
        );
        assert!(needs_reindex(&entry, &provider));
    }

    #[test]
    fn test_needs_reindex_false_when_matching() {
        let provider = FakeEmbeddingProvider::new(8);
        let entry = MemoryEntry::new(
            "hi",
            vec![0.0; 8],
            MemoryMetadata::new("ws"),
            provider.model_id(),
            provider.version(),
        );
        assert!(!needs_reindex(&entry, &provider));
    }
}
