//! Optional TurboVec-accelerated ANN backend.
//!
//! Feature-gated behind `turbovec`, OFF by default: unless this crate is
//! built with `--features turbovec`, this module does not exist and the
//! `turbovec` dependency is never compiled or linked (it is declared
//! `optional = true` in `Cargo.toml`).
//!
//! # Upstream API verification
//!
//! This module is written against the REAL, verified public API of
//! `turbovec = "0.9.0"` (<https://docs.rs/turbovec/0.9.0>,
//! <https://github.com/RyanCodrai/turbovec>). The crate was vendored via
//! `cargo` from crates.io into the local registry cache and its source
//! (`src/lib.rs`, `src/id_map.rs`, `src/error.rs`) was read directly
//! before writing a single line of this module — no type or method here
//! is invented. In particular, [`turbovec::IdMapIndex::search_with_allowlist`]
//! is exactly the "workspace allowlist filtering" primitive this backend
//! needs: turbovec's own vocabulary for "search restricted to a set of
//! external ids" *is* an allowlist.
//!
//! No performance claims are made: this backend has not been benchmarked
//! in this codebase (no benchmark).
//!
//! # Design: composition over reimplementation
//!
//! `turbovec::IdMapIndex` stores only `(u64 id, quantized vector)` pairs —
//! it has no notion of text, metadata, or workspace, and does not persist
//! anything but the vectors and ids. So this backend composes:
//!
//! - A [`ReferenceMemoryStore`] as the durable source of truth for text,
//!   metadata, and persistence — inheriting its schema versioning and
//!   fail-closed corruption handling for free.
//! - An in-memory `IdMapIndex`, rebuilt from the reference store's
//!   entries every time [`TurboVecMemoryStore::open`] runs, used only to
//!   accelerate `search`.
//!
//! `IdMapIndex` requires one fixed dimensionality (locked at
//! construction) and external `u64` ids. [`MemoryId`] is a 128-bit UUID,
//! so each id is mapped to a `u64` via the high 64 bits of the UUID
//! ([`uuid::Uuid::as_u64_pair`]); collisions are detected explicitly
//! (tracked in the `id_map` side table) rather than risking silently
//! mixed vectors.
//!
//! ## Dimension / model / version validation and fallback
//!
//! If the store contains any entry whose embedding dimensionality,
//! model id, or model version does not match this backend's
//! configuration, or if a `u64` id collision is detected, the ANN index
//! is dropped and marked [`IndexState::Unavailable`]; from that point
//! `search` transparently falls back to the reference backend's exact
//! brute-force cosine search until the store is reopened (which rebuilds
//! the index from scratch and can recover to `Ready` once every entry
//! matches). No data is ever lost by this fallback — it only affects
//! *whether* search is accelerated.
//!
//! `search` also falls back to the reference backend (rather than
//! filtering post-hoc, which would silently reduce recall for a top-k ANN
//! query) whenever the caller's [`MemoryQuery`] asks for tag or metadata
//! filtering that `IdMapIndex` cannot express — turbovec is only used for
//! its actual strength here: workspace-allowlisted vector search.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::path::Path;

use async_trait::async_trait;
use tokio::sync::Mutex;
use turbovec::IdMapIndex;

use crate::error::MemoryError;
use crate::reference::ReferenceMemoryStore;
use crate::traits::MemoryStore;
use crate::types::{MemoryEntry, MemoryId, MemoryQuery, MemorySearchResult};

/// Derive the `u64` id turbovec uses for `id`, from the high 64 bits of
/// the underlying UUID. Collisions between distinct `MemoryId`s are
/// possible in principle (birthday bound on 64 bits) and are detected
/// explicitly wherever this id is inserted into the index/id_map.
fn slot_id(id: MemoryId) -> u64 {
    id.0.as_u64_pair().0
}

/// State of the in-memory ANN index.
enum IndexState {
    /// The index exactly mirrors every entry in the reference store, all
    /// of which match this backend's configured dim/model/version, with
    /// no `u64` id collisions. Boxed: `IdMapIndex` is large relative to
    /// `Unavailable`, and this variant is already behind a `Mutex`.
    Ready {
        index: Box<IdMapIndex>,
        id_map: HashMap<u64, MemoryId>,
    },
    /// The index could not be (re)built or was invalidated by a
    /// mutation; `search` falls back to the reference backend. Carries a
    /// human-readable reason for diagnostics.
    Unavailable { reason: String },
}

/// Optional TurboVec-accelerated [`MemoryStore`].
///
/// Only available when this crate is built with `--features turbovec`.
/// See the module docs for the persistence and fallback design.
pub struct TurboVecMemoryStore {
    reference: ReferenceMemoryStore,
    dim: usize,
    embedding_model: String,
    embedding_version: u32,
    state: Mutex<IndexState>,
}

impl TurboVecMemoryStore {
    /// Open a store at `path`, accelerated by a turbovec ANN index over
    /// `dim`-dimensional vectors produced by `embedding_model` /
    /// `embedding_version`.
    ///
    /// `bit_width` must be 2, 3, or 4 (turbovec's supported quantization
    /// widths); anything else is rejected with [`MemoryError::Backend`].
    /// Persistence and corruption handling are entirely delegated to the
    /// underlying [`ReferenceMemoryStore`] (see its docs); the ANN index
    /// itself is rebuilt in memory from the reopened entries.
    pub async fn open(
        path: impl AsRef<Path>,
        dim: usize,
        bit_width: usize,
        embedding_model: impl Into<String>,
        embedding_version: u32,
    ) -> Result<Self, MemoryError> {
        if !(2..=4).contains(&bit_width) {
            return Err(MemoryError::Backend(format!(
                "turbovec bit_width must be 2, 3, or 4, got {bit_width}"
            )));
        }

        let reference = ReferenceMemoryStore::open(path)?;
        let embedding_model = embedding_model.into();
        let entries = reference.all_entries().await;
        let state = build_index(
            &entries,
            dim,
            bit_width,
            &embedding_model,
            embedding_version,
        );

        Ok(Self {
            reference,
            dim,
            embedding_model,
            embedding_version,
            state: Mutex::new(state),
        })
    }

    /// `true` if the ANN index is currently active (search is
    /// accelerated by turbovec); `false` if this backend has fallen back
    /// to the reference backend's brute-force search.
    pub async fn is_accelerated(&self) -> bool {
        matches!(*self.state.lock().await, IndexState::Ready { .. })
    }

    /// Human-readable reason the ANN index is currently unavailable
    /// (falling back to the reference backend), or `None` if it is
    /// active. Intended for diagnostics/logging.
    pub async fn fallback_reason(&self) -> Option<String> {
        match &*self.state.lock().await {
            IndexState::Ready { .. } => None,
            IndexState::Unavailable { reason } => Some(reason.clone()),
        }
    }

    fn matches_config(&self, entry: &MemoryEntry) -> bool {
        entry.embedding.len() == self.dim
            && entry.embedding_model == self.embedding_model
            && entry.embedding_version == self.embedding_version
    }
}

/// Build a fresh ANN index from `entries`, or explain why it can't be
/// built. Every entry must match `dim`/`embedding_model`/
/// `embedding_version` and produce a unique `u64` slot id, or the whole
/// index is deemed [`IndexState::Unavailable`] (see module docs).
fn build_index(
    entries: &[MemoryEntry],
    dim: usize,
    bit_width: usize,
    embedding_model: &str,
    embedding_version: u32,
) -> IndexState {
    for entry in entries {
        if entry.embedding.len() != dim
            || entry.embedding_model != embedding_model
            || entry.embedding_version != embedding_version
        {
            return IndexState::Unavailable {
                reason: format!(
                    "entry {} does not match configured dim={dim}/model={embedding_model}/version={embedding_version}; pending re-index",
                    entry.id
                ),
            };
        }
    }

    let mut index = match IdMapIndex::new(dim, bit_width) {
        Ok(index) => index,
        Err(e) => {
            return IndexState::Unavailable {
                reason: format!("construct: {e}"),
            };
        }
    };

    let mut id_map: HashMap<u64, MemoryId> = HashMap::with_capacity(entries.len());
    for entry in entries {
        let slot = slot_id(entry.id);
        if let Some(existing) = id_map.get(&slot) {
            if *existing != entry.id {
                return IndexState::Unavailable {
                    reason: format!(
                        "u64 id collision building turbovec index for {existing} and {}",
                        entry.id
                    ),
                };
            }
            continue;
        }
        if let Err(e) = index.add_with_ids(&entry.embedding, &[slot]) {
            return IndexState::Unavailable {
                reason: format!("add: {e}"),
            };
        }
        id_map.insert(slot, entry.id);
    }

    IndexState::Ready {
        index: Box::new(index),
        id_map,
    }
}

#[async_trait]
impl MemoryStore for TurboVecMemoryStore {
    async fn add(&self, entry: MemoryEntry) -> Result<MemoryId, MemoryError> {
        let id = self.reference.add(entry.clone()).await?;

        let mut state = self.state.lock().await;
        let fallback = match &mut *state {
            IndexState::Ready { index, id_map } => {
                if !self.matches_config(&entry) {
                    Some(format!(
                        "entry {id} embedding dim/model/version does not match this backend's configuration"
                    ))
                } else {
                    let slot = slot_id(id);
                    match id_map.entry(slot) {
                        Entry::Occupied(_) => Some(format!("u64 id collision adding entry {id}")),
                        Entry::Vacant(vacant) => {
                            if let Err(e) = index.add_with_ids(&entry.embedding, &[slot]) {
                                Some(format!("turbovec add failed: {e}"))
                            } else {
                                vacant.insert(id);
                                None
                            }
                        }
                    }
                }
            }
            IndexState::Unavailable { .. } => None,
        };
        if let Some(reason) = fallback {
            *state = IndexState::Unavailable { reason };
        }

        Ok(id)
    }

    async fn get(&self, id: MemoryId) -> Result<Option<MemoryEntry>, MemoryError> {
        self.reference.get(id).await
    }

    async fn update(&self, entry: MemoryEntry) -> Result<(), MemoryError> {
        self.reference.update(entry.clone()).await?;

        let mut state = self.state.lock().await;
        let fallback = match &mut *state {
            IndexState::Ready { index, id_map } => {
                let slot = slot_id(entry.id);
                let was_present = index.remove(slot);
                id_map.remove(&slot);
                if !was_present {
                    Some(format!(
                        "invariant violated: {} missing from turbovec index during update",
                        entry.id
                    ))
                } else if !self.matches_config(&entry) {
                    Some(format!(
                        "entry {} embedding dim/model/version does not match this backend's configuration",
                        entry.id
                    ))
                } else if let Err(e) = index.add_with_ids(&entry.embedding, &[slot]) {
                    Some(format!("turbovec add failed during update: {e}"))
                } else {
                    id_map.insert(slot, entry.id);
                    None
                }
            }
            IndexState::Unavailable { .. } => None,
        };
        if let Some(reason) = fallback {
            *state = IndexState::Unavailable { reason };
        }

        Ok(())
    }

    async fn delete(&self, id: MemoryId) -> Result<bool, MemoryError> {
        let deleted = self.reference.delete(id).await?;
        if deleted {
            let mut state = self.state.lock().await;
            if let IndexState::Ready { index, id_map } = &mut *state {
                let slot = slot_id(id);
                index.remove(slot);
                id_map.remove(&slot);
            }
        }
        Ok(deleted)
    }

    async fn search(&self, query: &MemoryQuery) -> Result<Vec<MemorySearchResult>, MemoryError> {
        let state = self.state.lock().await;

        let (index, id_map) = match &*state {
            IndexState::Ready { index, id_map }
                if query.tags.is_empty()
                    && query.metadata.is_empty()
                    && query.embedding.len() == self.dim =>
            {
                (index, id_map)
            }
            _ => {
                drop(state);
                return self.reference.search(query).await;
            }
        };

        let entries = self.reference.all_entries().await;
        let allowed: Vec<u64> = entries
            .iter()
            .filter(|e| e.metadata.workspace == query.workspace)
            .map(|e| slot_id(e.id))
            .collect();
        if allowed.is_empty() {
            return Ok(Vec::new());
        }

        let k = query.limit.max(1);
        let (scores, ids) = index.search_with_allowlist(&query.embedding, k, Some(&allowed));

        let by_id: HashMap<MemoryId, MemoryEntry> =
            entries.into_iter().map(|e| (e.id, e)).collect();
        let mut results = Vec::with_capacity(ids.len());
        for (score, slot) in scores.into_iter().zip(ids) {
            let Some(mem_id) = id_map.get(&slot) else {
                continue;
            };
            let Some(entry) = by_id.get(mem_id) else {
                continue;
            };
            results.push(MemorySearchResult {
                entry: entry.clone(),
                score,
            });
        }
        results.truncate(query.limit);
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::MemoryMetadata;
    use tempfile::tempdir;

    fn entry(text: &str, workspace: &str, embedding: Vec<f32>) -> MemoryEntry {
        MemoryEntry::new(
            text,
            embedding,
            MemoryMetadata::new(workspace),
            "fake-hash-v1",
            1,
        )
    }

    #[tokio::test]
    async fn test_add_get_update_delete_accelerated() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mem.tv.jsonl");
        let store = TurboVecMemoryStore::open(&path, 8, 4, "fake-hash-v1", 1)
            .await
            .unwrap();
        assert!(store.is_accelerated().await);

        let e = entry("hello", "ws", vec![1.0; 8]);
        let id = store.add(e.clone()).await.unwrap();
        assert!(store.is_accelerated().await);

        let fetched = store.get(id).await.unwrap().unwrap();
        assert_eq!(fetched.text, "hello");

        let mut updated = fetched.clone();
        updated.text = "hello updated".to_string();
        store.update(updated).await.unwrap();
        assert!(store.is_accelerated().await);
        assert_eq!(store.get(id).await.unwrap().unwrap().text, "hello updated");

        assert!(store.delete(id).await.unwrap());
        assert!(store.get(id).await.unwrap().is_none());
        assert!(store.is_accelerated().await);
    }

    #[tokio::test]
    async fn test_workspace_allowlist_isolation() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mem.tv.jsonl");
        let store = TurboVecMemoryStore::open(&path, 8, 4, "fake-hash-v1", 1)
            .await
            .unwrap();

        store
            .add(entry("secret-a", "workspace-a", vec![1.0; 8]))
            .await
            .unwrap();
        store
            .add(entry("secret-b", "workspace-b", vec![1.0; 8]))
            .await
            .unwrap();
        assert!(store.is_accelerated().await);

        let query_a = MemoryQuery::new("workspace-a", vec![1.0; 8], 10);
        let results_a = store.search(&query_a).await.unwrap();
        assert_eq!(results_a.len(), 1);
        assert_eq!(results_a[0].entry.text, "secret-a");
        assert!(!results_a.iter().any(|r| r.entry.text == "secret-b"));

        let query_b = MemoryQuery::new("workspace-b", vec![1.0; 8], 10);
        let results_b = store.search(&query_b).await.unwrap();
        assert_eq!(results_b.len(), 1);
        assert_eq!(results_b[0].entry.text, "secret-b");
        assert!(!results_b.iter().any(|r| r.entry.text == "secret-a"));
    }

    #[tokio::test]
    async fn test_stale_dimension_falls_back_to_reference() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mem.tv.jsonl");
        let store = TurboVecMemoryStore::open(&path, 8, 4, "fake-hash-v1", 1)
            .await
            .unwrap();

        // Add an entry embedded with a different model/version than the
        // backend is configured for.
        let stale = MemoryEntry::new(
            "stale",
            vec![1.0; 8],
            MemoryMetadata::new("ws"),
            "some-other-model",
            99,
        );
        store.add(stale.clone()).await.unwrap();
        assert!(!store.is_accelerated().await);
        assert!(store.fallback_reason().await.is_some());

        // Search must still work correctly via fallback, and the stale
        // entry must still be retrievable.
        let query = MemoryQuery::new("ws", vec![1.0; 8], 10);
        let results = store.search(&query).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entry.text, "stale");
    }

    #[tokio::test]
    async fn test_reopen_rebuilds_index_and_persists() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mem.tv.jsonl");

        let id = {
            let store = TurboVecMemoryStore::open(&path, 8, 4, "fake-hash-v1", 1)
                .await
                .unwrap();
            let e = entry("durable", "ws", vec![1.0; 8]);
            let id = store.add(e.clone()).await.unwrap();
            assert!(store.is_accelerated().await);
            id
        };

        let reopened = TurboVecMemoryStore::open(&path, 8, 4, "fake-hash-v1", 1)
            .await
            .unwrap();
        assert!(reopened.is_accelerated().await);
        assert_eq!(reopened.get(id).await.unwrap().unwrap().text, "durable");

        let query = MemoryQuery::new("ws", vec![1.0; 8], 10);
        let results = reopened.search(&query).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entry.text, "durable");
    }

    #[tokio::test]
    async fn test_invalid_bit_width_rejected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mem.tv.jsonl");
        // `TurboVecMemoryStore` intentionally does not implement `Debug`
        // (its `IdMapIndex` field doesn't either), so match on the
        // `Result` directly instead of calling `unwrap_err()`.
        match TurboVecMemoryStore::open(&path, 8, 5, "fake-hash-v1", 1).await {
            Err(MemoryError::Backend(_)) => {}
            Err(other) => panic!("expected MemoryError::Backend, got {other:?}"),
            Ok(_) => panic!("expected an error for bit_width=5"),
        }
    }

    #[tokio::test]
    async fn test_tag_filtered_query_falls_back_to_reference() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mem.tv.jsonl");
        let store = TurboVecMemoryStore::open(&path, 8, 4, "fake-hash-v1", 1)
            .await
            .unwrap();

        let mut tagged = entry("tagged", "ws", vec![1.0; 8]);
        tagged.metadata = tagged.metadata.with_tags(["urgent"]);
        store.add(tagged).await.unwrap();
        store.add(entry("plain", "ws", vec![1.0; 8])).await.unwrap();
        assert!(store.is_accelerated().await);

        let query = MemoryQuery::new("ws", vec![1.0; 8], 10).with_tags(["urgent"]);
        let results = store.search(&query).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entry.text, "tagged");
        // Falling back for a tag-filtered query must not disturb
        // acceleration for subsequent unfiltered queries.
        assert!(store.is_accelerated().await);
    }
}
