//! Core data types shared by every memory backend.

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Stable identifier for a memory entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MemoryId(pub Uuid);

impl MemoryId {
    /// Generate a fresh random identifier.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for MemoryId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for MemoryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for MemoryId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

/// Metadata attached to a memory entry.
///
/// `workspace` is the isolation boundary: every `MemoryStore::search` call
/// is scoped to exactly one workspace and must never return entries from a
/// different workspace (see `MemoryQuery::workspace`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MemoryMetadata {
    /// Isolation boundary for this entry (e.g. a project or tenant id).
    pub workspace: String,
    /// Free-form tags, matched by `MemoryQuery::tags` (any-of).
    #[serde(default)]
    pub tags: Vec<String>,
    /// Optional citation source (e.g. a document id/path) so retrieved
    /// entries can be traced back to where they came from.
    #[serde(default)]
    pub source: Option<String>,
    /// Arbitrary caller-defined key/value metadata, matched exactly by
    /// `MemoryQuery::metadata`.
    #[serde(default)]
    pub extra: HashMap<String, String>,
}

impl MemoryMetadata {
    pub fn new(workspace: impl Into<String>) -> Self {
        Self {
            workspace: workspace.into(),
            ..Default::default()
        }
    }

    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    pub fn with_tags(mut self, tags: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.tags = tags.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_extra(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra.insert(key.into(), value.into());
        self
    }
}

/// A single stored memory: text, its embedding, and metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: MemoryId,
    pub text: String,
    pub embedding: Vec<f32>,
    pub metadata: MemoryMetadata,
    /// Identifier of the embedding model that produced `embedding`. Used to
    /// detect stale entries when the embedding model changes.
    pub embedding_model: String,
    /// Version of the embedding model that produced `embedding`.
    pub embedding_version: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl MemoryEntry {
    /// Construct a new entry with a fresh id and `created_at == updated_at`.
    pub fn new(
        text: impl Into<String>,
        embedding: Vec<f32>,
        metadata: MemoryMetadata,
        embedding_model: impl Into<String>,
        embedding_version: u32,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: MemoryId::new(),
            text: text.into(),
            embedding,
            metadata,
            embedding_model: embedding_model.into(),
            embedding_version,
            created_at: now,
            updated_at: now,
        }
    }
}

/// A search request against a `MemoryStore`.
///
/// `workspace` is required (not optional) so that callers cannot
/// accidentally issue an unscoped, cross-workspace search.
#[derive(Debug, Clone)]
pub struct MemoryQuery {
    /// Isolation boundary: only entries with a matching
    /// `MemoryMetadata::workspace` are ever considered.
    pub workspace: String,
    /// Query embedding to rank candidates by similarity.
    pub embedding: Vec<f32>,
    /// Maximum number of results to return.
    pub limit: usize,
    /// If non-empty, an entry must have at least one of these tags.
    pub tags: Vec<String>,
    /// If non-empty, an entry's `MemoryMetadata::extra` must contain every
    /// key/value pair listed here (exact match).
    pub metadata: HashMap<String, String>,
}

impl MemoryQuery {
    pub fn new(workspace: impl Into<String>, embedding: Vec<f32>, limit: usize) -> Self {
        Self {
            workspace: workspace.into(),
            embedding,
            limit,
            tags: Vec::new(),
            metadata: HashMap::new(),
        }
    }

    pub fn with_tags(mut self, tags: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.tags = tags.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }
}

/// A single scored search result.
#[derive(Debug, Clone, PartialEq)]
pub struct MemorySearchResult {
    pub entry: MemoryEntry,
    /// Similarity score; higher is more relevant. Backends define their
    /// own scale (the reference backend uses cosine similarity, [-1, 1]).
    pub score: f32,
}

/// Cosine similarity between two vectors of equal length.
///
/// Returns `0.0` if either vector is empty or all-zero (a degenerate
/// but non-panicking result) rather than dividing by zero.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len(), "cosine_similarity: dimension mismatch");
    let mut dot = 0.0f64;
    let mut norm_a = 0.0f64;
    let mut norm_b = 0.0f64;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += (*x as f64) * (*y as f64);
        norm_a += (*x as f64) * (*x as f64);
        norm_b += (*y as f64) * (*y as f64);
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    (dot / (norm_a.sqrt() * norm_b.sqrt())) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_id_display_roundtrip() {
        let id = MemoryId::new();
        let s = id.to_string();
        let parsed: MemoryId = s.parse().unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn test_cosine_similarity_identical_vectors_is_one() {
        let v = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&v, &v);
        assert!((sim - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_orthogonal_is_zero() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_zero_vector_is_zero_not_nan() {
        let a = vec![0.0, 0.0];
        let b = vec![1.0, 2.0];
        let sim = cosine_similarity(&a, &b);
        assert_eq!(sim, 0.0);
    }

    #[test]
    fn test_memory_entry_new_sets_timestamps_equal() {
        let entry = MemoryEntry::new(
            "hello",
            vec![0.1, 0.2],
            MemoryMetadata::new("ws-a"),
            "fake-hash-v1",
            1,
        );
        assert_eq!(entry.created_at, entry.updated_at);
        assert_eq!(entry.metadata.workspace, "ws-a");
    }
}
