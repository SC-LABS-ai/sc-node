//! Reference `MemoryStore` backend: an append-only JSONL log with an
//! in-memory index rebuilt on load.
//!
//! # Persistence format choice
//!
//! This machine builds with the GNU toolchain and no reliable C compiler,
//! which rules out `rusqlite` with the bundled C SQLite. Rather than pull
//! in an unfamiliar pure-Rust embedded database (e.g. `redb`) whose exact
//! API/behaviour we would need to trust unverified, this backend uses the
//! same append-only-JSONL-plus-replay approach already used elsewhere in
//! this codebase (see `sc-audit::AuditLogger`): every mutation is appended
//! as one JSON line to a log file, and the current state is rebuilt by
//! replaying the log from the top. This is:
//!
//! - Pure Rust, zero new heavyweight dependencies (just `serde_json`,
//!   already a workspace dependency).
//! - Simple enough to make corruption handling precise: a malformed line
//!   is reported with its 1-based line number, and the whole load fails
//!   closed rather than silently dropping data or panicking.
//! - Simple enough to make schema versioning explicit: the first line of
//!   every file is a `Header` record carrying a schema version, checked
//!   against `SCHEMA_VERSION` before any data record is trusted.
//! - Human-inspectable and diffable, which matters for a local-first,
//!   privacy-sensitive tool.
//!
//! The tradeoff is O(n) load time and O(file size) disk usage relative to
//! a real transactional KV store; for a local memory store sized for a
//! single user's session history this is an acceptable and, given the
//! toolchain constraints, safer choice than an unverified dependency.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::error::MemoryError;
use crate::traits::MemoryStore;
use crate::types::{MemoryEntry, MemoryId, MemoryQuery, MemorySearchResult, cosine_similarity};

/// Current on-disk schema version. Bump this (and add migration/rejection
/// logic) whenever the `LogRecord` shape changes in a way that is not
/// backward compatible.
pub const SCHEMA_VERSION: u32 = 1;

/// One line of the append-only log.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "record", rename_all = "snake_case")]
enum LogRecord {
    /// Always the first line of a well-formed file.
    Header {
        schema_version: u32,
    },
    Add {
        entry: MemoryEntry,
    },
    Update {
        entry: MemoryEntry,
    },
    Delete {
        id: MemoryId,
    },
}

#[derive(Debug)]
struct StoreState {
    entries: HashMap<MemoryId, MemoryEntry>,
    file: std::fs::File,
}

/// Persistent, pure-Rust reference implementation of [`MemoryStore`].
///
/// Backed by a single append-only JSONL file at `path`. Safe to reopen
/// after a process restart: [`Self::open`] replays the log to rebuild the
/// in-memory index before returning.
#[derive(Debug)]
pub struct ReferenceMemoryStore {
    path: PathBuf,
    state: Mutex<StoreState>,
}

impl ReferenceMemoryStore {
    /// Open (creating if necessary) a store backed by the JSONL file at
    /// `path`.
    ///
    /// - If `path` does not exist, a new file is created with a fresh
    ///   `Header` record.
    /// - If `path` exists, it is fully replayed. A malformed header or
    ///   data line yields [`MemoryError::Corrupt`] with the offending
    ///   line number rather than panicking. A header declaring an
    ///   unrecognized schema version yields
    ///   [`MemoryError::UnsupportedSchemaVersion`].
    pub fn open(path: impl AsRef<Path>) -> Result<Self, MemoryError> {
        let path = path.as_ref().to_path_buf();

        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }

        let entries = if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            replay(&content)?
        } else {
            HashMap::new()
        };

        let mut file = OpenOptions::new().create(true).append(true).open(&path)?;

        if !path_has_content(&path)? {
            write_record(
                &mut file,
                &LogRecord::Header {
                    schema_version: SCHEMA_VERSION,
                },
            )?;
        }

        Ok(Self {
            path,
            state: Mutex::new(StoreState { entries, file }),
        })
    }

    /// Path to the backing JSONL file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Snapshot of every entry currently in the store, in no particular
    /// order.
    ///
    /// Intended for backends that need to build a derived structure from
    /// the reference backend's durable state (e.g. the optional turbovec
    /// backend rebuilding its ANN index), not for general-purpose
    /// iteration by application code.
    pub async fn all_entries(&self) -> Vec<MemoryEntry> {
        let state = self.state.lock().await;
        state.entries.values().cloned().collect()
    }
}

fn path_has_content(path: &Path) -> Result<bool, MemoryError> {
    Ok(path.metadata()?.len() > 0)
}

/// Replay a full log file's contents into an in-memory index.
///
/// Fails closed: any malformed header or data line aborts the whole load
/// with a [`MemoryError::Corrupt`] (including the 1-based line number) or
/// [`MemoryError::UnsupportedSchemaVersion`], rather than returning
/// partial data or panicking.
fn replay(content: &str) -> Result<HashMap<MemoryId, MemoryEntry>, MemoryError> {
    let mut lines = content.lines().enumerate();
    let mut entries = HashMap::new();

    let Some((_, header_line)) = lines.find(|(_, l)| !l.trim().is_empty()) else {
        // Empty file (or all-blank): treat as a fresh, empty store.
        return Ok(entries);
    };

    match serde_json::from_str::<LogRecord>(header_line) {
        Ok(LogRecord::Header { schema_version }) => {
            if schema_version != SCHEMA_VERSION {
                return Err(MemoryError::UnsupportedSchemaVersion {
                    found: schema_version,
                    supported: SCHEMA_VERSION,
                });
            }
        }
        Ok(_) => {
            return Err(MemoryError::Corrupt {
                line: 1,
                reason: "expected a header record as the first line".to_string(),
            });
        }
        Err(e) => {
            return Err(MemoryError::Corrupt {
                line: 1,
                reason: format!("invalid header line: {e}"),
            });
        }
    }

    for (idx, line) in lines {
        if line.trim().is_empty() {
            continue;
        }
        let record: LogRecord = serde_json::from_str(line).map_err(|e| MemoryError::Corrupt {
            line: idx + 1,
            reason: format!("invalid record: {e}"),
        })?;
        match record {
            LogRecord::Header { .. } => {
                return Err(MemoryError::Corrupt {
                    line: idx + 1,
                    reason: "unexpected second header record".to_string(),
                });
            }
            LogRecord::Add { entry } | LogRecord::Update { entry } => {
                entries.insert(entry.id, entry);
            }
            LogRecord::Delete { id } => {
                entries.remove(&id);
            }
        }
    }

    Ok(entries)
}

fn write_record(file: &mut std::fs::File, record: &LogRecord) -> Result<(), MemoryError> {
    let mut line = serde_json::to_string(record)?;
    line.push('\n');
    file.write_all(line.as_bytes())?;
    file.flush()?;
    Ok(())
}

#[async_trait]
impl MemoryStore for ReferenceMemoryStore {
    async fn add(&self, entry: MemoryEntry) -> Result<MemoryId, MemoryError> {
        let mut state = self.state.lock().await;
        if state.entries.contains_key(&entry.id) {
            return Err(MemoryError::AlreadyExists(entry.id));
        }
        write_record(
            &mut state.file,
            &LogRecord::Add {
                entry: entry.clone(),
            },
        )?;
        let id = entry.id;
        state.entries.insert(id, entry);
        Ok(id)
    }

    async fn get(&self, id: MemoryId) -> Result<Option<MemoryEntry>, MemoryError> {
        let state = self.state.lock().await;
        Ok(state.entries.get(&id).cloned())
    }

    async fn update(&self, entry: MemoryEntry) -> Result<(), MemoryError> {
        let mut state = self.state.lock().await;
        if !state.entries.contains_key(&entry.id) {
            return Err(MemoryError::NotFound(entry.id));
        }
        write_record(
            &mut state.file,
            &LogRecord::Update {
                entry: entry.clone(),
            },
        )?;
        state.entries.insert(entry.id, entry);
        Ok(())
    }

    async fn delete(&self, id: MemoryId) -> Result<bool, MemoryError> {
        let mut state = self.state.lock().await;
        if !state.entries.contains_key(&id) {
            return Ok(false);
        }
        write_record(&mut state.file, &LogRecord::Delete { id })?;
        state.entries.remove(&id);
        Ok(true)
    }

    async fn search(&self, query: &MemoryQuery) -> Result<Vec<MemorySearchResult>, MemoryError> {
        let state = self.state.lock().await;

        let mut results: Vec<MemorySearchResult> = state
            .entries
            .values()
            .filter(|entry| entry.metadata.workspace == query.workspace)
            .filter(|entry| {
                query.tags.is_empty() || query.tags.iter().any(|t| entry.metadata.tags.contains(t))
            })
            .filter(|entry| {
                query
                    .metadata
                    .iter()
                    .all(|(k, v)| entry.metadata.extra.get(k) == Some(v))
            })
            .filter(|entry| entry.embedding.len() == query.embedding.len())
            .map(|entry| MemorySearchResult {
                entry: entry.clone(),
                score: cosine_similarity(&entry.embedding, &query.embedding),
            })
            .collect();

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
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
    async fn test_add_get_update_delete() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mem.jsonl");
        let store = ReferenceMemoryStore::open(&path).unwrap();

        let e = entry("hello", "ws", vec![1.0, 0.0]);
        let id = store.add(e.clone()).await.unwrap();
        assert_eq!(id, e.id);

        let fetched = store.get(id).await.unwrap().unwrap();
        assert_eq!(fetched.text, "hello");

        let mut updated = fetched.clone();
        updated.text = "hello updated".to_string();
        store.update(updated.clone()).await.unwrap();
        let fetched2 = store.get(id).await.unwrap().unwrap();
        assert_eq!(fetched2.text, "hello updated");

        let deleted = store.delete(id).await.unwrap();
        assert!(deleted);
        assert!(store.get(id).await.unwrap().is_none());

        // Deleting again is a no-op, not an error.
        let deleted_again = store.delete(id).await.unwrap();
        assert!(!deleted_again);
    }

    #[tokio::test]
    async fn test_all_entries_snapshot() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mem.jsonl");
        let store = ReferenceMemoryStore::open(&path).unwrap();

        store.add(entry("a", "ws", vec![1.0, 0.0])).await.unwrap();
        store.add(entry("b", "ws", vec![0.0, 1.0])).await.unwrap();

        let mut texts: Vec<String> = store
            .all_entries()
            .await
            .into_iter()
            .map(|e| e.text)
            .collect();
        texts.sort();
        assert_eq!(texts, vec!["a".to_string(), "b".to_string()]);
    }

    #[tokio::test]
    async fn test_add_duplicate_id_rejected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mem.jsonl");
        let store = ReferenceMemoryStore::open(&path).unwrap();

        let e = entry("hello", "ws", vec![1.0, 0.0]);
        store.add(e.clone()).await.unwrap();
        let err = store.add(e.clone()).await.unwrap_err();
        assert!(matches!(err, MemoryError::AlreadyExists(_)));
    }

    #[tokio::test]
    async fn test_update_missing_is_not_found() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mem.jsonl");
        let store = ReferenceMemoryStore::open(&path).unwrap();

        let e = entry("hello", "ws", vec![1.0, 0.0]);
        let err = store.update(e).await.unwrap_err();
        assert!(matches!(err, MemoryError::NotFound(_)));
    }

    #[tokio::test]
    async fn test_metadata_filter() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mem.jsonl");
        let store = ReferenceMemoryStore::open(&path).unwrap();

        let mut a = entry("a", "ws", vec![1.0, 0.0]);
        a.metadata = a.metadata.with_extra("kind", "note");
        let mut b = entry("b", "ws", vec![1.0, 0.0]);
        b.metadata = b.metadata.with_extra("kind", "todo");
        store.add(a.clone()).await.unwrap();
        store.add(b.clone()).await.unwrap();

        let query = MemoryQuery::new("ws", vec![1.0, 0.0], 10).with_metadata("kind", "note");
        let results = store.search(&query).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entry.text, "a");
    }

    #[tokio::test]
    async fn test_tag_filter() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mem.jsonl");
        let store = ReferenceMemoryStore::open(&path).unwrap();

        let mut a = entry("a", "ws", vec![1.0, 0.0]);
        a.metadata = a.metadata.with_tags(["urgent"]);
        let b = entry("b", "ws", vec![1.0, 0.0]);
        store.add(a).await.unwrap();
        store.add(b).await.unwrap();

        let query = MemoryQuery::new("ws", vec![1.0, 0.0], 10).with_tags(["urgent"]);
        let results = store.search(&query).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entry.text, "a");
    }

    #[tokio::test]
    async fn test_workspace_filter_isolates_entries() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mem.jsonl");
        let store = ReferenceMemoryStore::open(&path).unwrap();

        store
            .add(entry("secret-a", "workspace-a", vec![1.0, 0.0]))
            .await
            .unwrap();
        store
            .add(entry("secret-b", "workspace-b", vec![1.0, 0.0]))
            .await
            .unwrap();

        let query_a = MemoryQuery::new("workspace-a", vec![1.0, 0.0], 10);
        let results_a = store.search(&query_a).await.unwrap();
        assert_eq!(results_a.len(), 1);
        assert_eq!(results_a[0].entry.text, "secret-a");

        let query_b = MemoryQuery::new("workspace-b", vec![1.0, 0.0], 10);
        let results_b = store.search(&query_b).await.unwrap();
        assert_eq!(results_b.len(), 1);
        assert_eq!(results_b[0].entry.text, "secret-b");

        // Entries from workspace A must never appear for workspace B and
        // vice versa.
        assert!(!results_b.iter().any(|r| r.entry.text == "secret-a"));
        assert!(!results_a.iter().any(|r| r.entry.text == "secret-b"));
    }

    #[tokio::test]
    async fn test_persistence_across_reopen() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mem.jsonl");

        let id = {
            let store = ReferenceMemoryStore::open(&path).unwrap();
            let e = entry("durable", "ws", vec![1.0, 0.0]);
            store.add(e.clone()).await.unwrap();
            e.id
        };
        // `store` dropped here, simulating a process restart.

        let reopened = ReferenceMemoryStore::open(&path).unwrap();
        let fetched = reopened.get(id).await.unwrap().unwrap();
        assert_eq!(fetched.text, "durable");
    }

    #[tokio::test]
    async fn test_persistence_survives_update_and_delete_across_reopen() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mem.jsonl");

        let (keep_id, deleted_id) = {
            let store = ReferenceMemoryStore::open(&path).unwrap();
            let a = entry("keep", "ws", vec![1.0, 0.0]);
            let b = entry("gone", "ws", vec![0.0, 1.0]);
            store.add(a.clone()).await.unwrap();
            store.add(b.clone()).await.unwrap();
            store.delete(b.id).await.unwrap();
            let mut updated = a.clone();
            updated.text = "keep updated".to_string();
            store.update(updated).await.unwrap();
            (a.id, b.id)
        };

        let reopened = ReferenceMemoryStore::open(&path).unwrap();
        assert_eq!(
            reopened.get(keep_id).await.unwrap().unwrap().text,
            "keep updated"
        );
        assert!(reopened.get(deleted_id).await.unwrap().is_none());
    }

    #[test]
    fn test_corrupt_garbage_file_yields_error_not_panic() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mem.jsonl");
        std::fs::write(&path, b"this is not json at all\n{{{garbage").unwrap();

        let result = ReferenceMemoryStore::open(&path);
        assert!(result.is_err());
        match result.unwrap_err() {
            MemoryError::Corrupt { line, .. } => assert_eq!(line, 1),
            other => panic!("expected Corrupt, got {other:?}"),
        }
    }

    #[test]
    fn test_corrupt_data_line_after_valid_header_yields_error() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mem.jsonl");
        let content = format!(
            "{}\nnot valid json\n",
            serde_json::to_string(&LogRecord::Header {
                schema_version: SCHEMA_VERSION
            })
            .unwrap()
        );
        std::fs::write(&path, content).unwrap();

        let result = ReferenceMemoryStore::open(&path);
        assert!(result.is_err());
        match result.unwrap_err() {
            MemoryError::Corrupt { line, .. } => assert_eq!(line, 2),
            other => panic!("expected Corrupt, got {other:?}"),
        }
    }

    #[test]
    fn test_unsupported_schema_version_is_rejected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mem.jsonl");
        let content = serde_json::to_string(&LogRecord::Header {
            schema_version: 999,
        })
        .unwrap();
        std::fs::write(&path, content + "\n").unwrap();

        let result = ReferenceMemoryStore::open(&path);
        match result {
            Err(MemoryError::UnsupportedSchemaVersion { found, supported }) => {
                assert_eq!(found, 999);
                assert_eq!(supported, SCHEMA_VERSION);
            }
            other => panic!("expected UnsupportedSchemaVersion, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_dimension_mismatch_entries_are_skipped_not_erroring() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mem.jsonl");
        let store = ReferenceMemoryStore::open(&path).unwrap();

        store
            .add(entry("two-dim", "ws", vec![1.0, 0.0]))
            .await
            .unwrap();
        store
            .add(entry("three-dim", "ws", vec![1.0, 0.0, 0.0]))
            .await
            .unwrap();

        let query = MemoryQuery::new("ws", vec![1.0, 0.0], 10);
        let results = store.search(&query).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entry.text, "two-dim");
    }
}
