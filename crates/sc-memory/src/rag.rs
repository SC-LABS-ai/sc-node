//! Minimal, deterministic local RAG (retrieval-augmented generation)
//! pipeline: chunk -> embed -> persist -> retrieve -> fake-answer.
//!
//! Every piece here is deterministic and local-only (no network): the
//! chunker is a pure function, the embedding provider used in tests is
//! [`crate::FakeEmbeddingProvider`] (SHA-256 hashing), and [`fake_answer`]
//! is a citation template, not a real LLM call. This module exists to
//! prove the end-to-end wiring — chunk -> embed -> persist -> restart ->
//! workspace-filtered retrieve -> answer-with-citation — not to produce
//! useful answers.

use crate::types::MemorySearchResult;

/// Split `text` into overlapping chunks of at most `chunk_size` words,
/// each chunk overlapping the previous one by `overlap` words.
///
/// Deterministic and pure (no I/O, no randomness). Returns an empty
/// vec for empty/whitespace-only `text` or `chunk_size == 0`. `overlap`
/// is clamped to `chunk_size - 1` so the chunker always advances (an
/// overlap `>= chunk_size` would otherwise loop forever).
pub fn chunk_text(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() || chunk_size == 0 {
        return Vec::new();
    }
    let overlap = overlap.min(chunk_size.saturating_sub(1));
    let step = chunk_size - overlap;

    let mut chunks = Vec::new();
    let mut start = 0;
    loop {
        let end = (start + chunk_size).min(words.len());
        chunks.push(words[start..end].join(" "));
        if end == words.len() {
            break;
        }
        start += step;
    }
    chunks
}

/// A deterministic, template-based "answer", standing in for a real
/// generation model so the RAG proof never touches the network.
#[derive(Debug, Clone, PartialEq)]
pub struct FakeAnswer {
    pub question: String,
    pub text: String,
    /// Citation for the best-matching chunk: its
    /// [`crate::MemoryMetadata::source`] if one was recorded, otherwise
    /// its [`crate::MemoryId`] rendered as a string.
    pub source_id: String,
}

/// Produce a deterministic answer to `question` from the top-ranked
/// `results` (already retrieved and workspace-filtered by a
/// `MemoryStore::search` call). The answer always cites the source id of
/// the highest-scoring result. Returns `None` if `results` is empty —
/// there is nothing to answer from.
pub fn fake_answer(question: &str, results: &[MemorySearchResult]) -> Option<FakeAnswer> {
    let top = results.first()?;
    let source_id = top
        .entry
        .metadata
        .source
        .clone()
        .unwrap_or_else(|| top.entry.id.to_string());
    Some(FakeAnswer {
        question: question.to_string(),
        text: format!("[source:{source_id}] {}", top.entry.text),
        source_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_embedding::FakeEmbeddingProvider;
    use crate::reference::ReferenceMemoryStore;
    use crate::traits::{EmbeddingProvider, MemoryStore, needs_reindex};
    use crate::types::{MemoryEntry, MemoryMetadata, MemoryQuery};
    use tempfile::tempdir;

    #[test]
    fn test_chunk_text_basic_overlap() {
        let text = "one two three four five six";
        let chunks = chunk_text(text, 3, 1);
        assert_eq!(chunks, vec!["one two three", "three four five", "five six"]);
    }

    #[test]
    fn test_chunk_text_no_overlap() {
        let chunks = chunk_text("a b c d", 2, 0);
        assert_eq!(chunks, vec!["a b", "c d"]);
    }

    #[test]
    fn test_chunk_text_empty_input() {
        assert_eq!(chunk_text("   ", 3, 1), Vec::<String>::new());
        assert_eq!(chunk_text("anything", 0, 0), Vec::<String>::new());
    }

    #[test]
    fn test_chunk_text_overlap_clamped_still_advances() {
        // overlap >= chunk_size would never advance without clamping.
        let chunks = chunk_text("a b c d e", 2, 5);
        assert_eq!(chunks, vec!["a b", "b c", "c d", "d e"]);
    }

    #[test]
    fn test_fake_answer_none_when_no_results() {
        assert!(fake_answer("anything?", &[]).is_none());
    }

    /// Proof test: index a small deterministic fixture into two
    /// workspaces, restart the store, ask a known question scoped to one
    /// workspace, and verify:
    /// - only the allowed workspace's chunks are ever retrieved,
    /// - the fake-model answer cites the correct source id,
    /// - the foreign workspace's document is never retrieved, even when
    ///   queried with its own content's embedding under the wrong scope.
    #[tokio::test]
    async fn test_end_to_end_rag_workflow_restart_and_workspace_isolation() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("rag.jsonl");
        let provider = FakeEmbeddingProvider::new(32);

        let doc_a = "Fable Five is a local-first AI agent runtime written in Rust. \
                     It never sends data off the local machine.";
        let doc_b = "This is a confidential document belonging to a different \
                     workspace and must stay isolated from workspace-allowed.";

        // --- Index phase ---
        {
            let store = ReferenceMemoryStore::open(&path).unwrap();

            for chunk in chunk_text(doc_a, 12, 3) {
                let embedding = provider.embed(&chunk).await.unwrap();
                let metadata = MemoryMetadata::new("workspace-allowed").with_source("doc-a");
                let entry = MemoryEntry::new(
                    chunk,
                    embedding,
                    metadata,
                    provider.model_id(),
                    provider.version(),
                );
                store.add(entry).await.unwrap();
            }

            for chunk in chunk_text(doc_b, 12, 3) {
                let embedding = provider.embed(&chunk).await.unwrap();
                let metadata = MemoryMetadata::new("workspace-foreign").with_source("doc-b");
                let entry = MemoryEntry::new(
                    chunk,
                    embedding,
                    metadata,
                    provider.model_id(),
                    provider.version(),
                );
                store.add(entry).await.unwrap();
            }
        }
        // `store` dropped here: simulates a process restart before we
        // ever query it.

        // --- Restart + retrieval phase ---
        let store = ReferenceMemoryStore::open(&path).unwrap();

        let question = "What is Fable Five written in?";
        let query_embedding = provider.embed(question).await.unwrap();
        let query = MemoryQuery::new("workspace-allowed", query_embedding, 3);
        let results = store.search(&query).await.unwrap();

        assert!(!results.is_empty(), "expected at least one retrieved chunk");
        for r in &results {
            assert_eq!(
                r.entry.metadata.source.as_deref(),
                Some("doc-a"),
                "workspace-allowed search returned a chunk not sourced from doc-a"
            );
        }

        let answer = fake_answer(question, &results).unwrap();
        assert_eq!(answer.source_id, "doc-a");
        assert!(answer.text.contains("doc-a"));

        // The foreign workspace's document must never be retrievable
        // under the allowed workspace's scope, even when queried with an
        // embedding of its own content.
        let leak_probe_embedding = provider.embed("confidential document").await.unwrap();
        let leak_probe = MemoryQuery::new("workspace-allowed", leak_probe_embedding, 10);
        let leak_results = store.search(&leak_probe).await.unwrap();
        assert!(
            leak_results
                .iter()
                .all(|r| r.entry.metadata.source.as_deref() != Some("doc-b")),
            "foreign-workspace document leaked into workspace-allowed retrieval"
        );

        // And querying the foreign workspace directly must never surface
        // doc-a either (isolation holds both directions).
        let foreign_query = MemoryQuery::new(
            "workspace-foreign",
            provider.embed(question).await.unwrap(),
            10,
        );
        let foreign_results = store.search(&foreign_query).await.unwrap();
        assert!(
            foreign_results
                .iter()
                .all(|r| r.entry.metadata.source.as_deref() != Some("doc-a")),
            "workspace-allowed document leaked into workspace-foreign retrieval"
        );
    }

    /// Deletion must propagate: a deleted chunk is immediately excluded
    /// from search, and stays excluded (get returns None) across a
    /// store restart.
    #[tokio::test]
    async fn test_deletion_propagates_across_search_and_restart() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("rag_delete.jsonl");
        let provider = FakeEmbeddingProvider::new(16);
        let store = ReferenceMemoryStore::open(&path).unwrap();

        let text = "a deletable chunk of text";
        let embedding = provider.embed(text).await.unwrap();
        let metadata = MemoryMetadata::new("ws").with_source("doc-x");
        let entry = MemoryEntry::new(
            text,
            embedding.clone(),
            metadata,
            provider.model_id(),
            provider.version(),
        );
        let id = store.add(entry).await.unwrap();

        let query = MemoryQuery::new("ws", embedding, 5);
        assert_eq!(store.search(&query).await.unwrap().len(), 1);

        assert!(store.delete(id).await.unwrap());
        assert!(store.search(&query).await.unwrap().is_empty());
        assert!(store.get(id).await.unwrap().is_none());

        drop(store);
        let reopened = ReferenceMemoryStore::open(&path).unwrap();
        assert!(reopened.get(id).await.unwrap().is_none());
        assert!(reopened.search(&query).await.unwrap().is_empty());
    }

    /// When the embedding model/version changes, previously-stored
    /// entries must be flagged as needing re-embedding rather than
    /// silently compared against incompatible fresh queries.
    #[tokio::test]
    async fn test_reindex_detected_on_embedding_model_or_version_change() {
        let old_provider = FakeEmbeddingProvider::new(16);
        let embedding = old_provider.embed("some indexed text").await.unwrap();
        let entry = MemoryEntry::new(
            "some indexed text",
            embedding,
            MemoryMetadata::new("ws"),
            old_provider.model_id(),
            old_provider.version(),
        );

        // Same provider config: no reindex needed.
        assert!(!needs_reindex(&entry, &old_provider));

        // A version bump on the same model id: stale.
        let bumped_provider = FakeEmbeddingProvider::with_model(
            16,
            old_provider.model_id(),
            old_provider.version() + 1,
        );
        assert!(needs_reindex(&entry, &bumped_provider));

        // A different model entirely: stale.
        let new_model_provider = FakeEmbeddingProvider::with_model(16, "fake-hash-v2", 1);
        assert!(needs_reindex(&entry, &new_model_provider));
    }
}
