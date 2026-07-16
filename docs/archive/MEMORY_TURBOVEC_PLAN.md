> **ARCHIVED / HISTORICAL — may be outdated.** Kept for context only; see
> [../ROADMAP.md](../ROADMAP.md) for current direction on memory/RAG. Original follows.

---

# SC Node — TurboVec Memory Plan

> **Status:** Evaluation / Planning — No implementation yet
> **Date:** 2026-07-09 · **Branch:** main

---

## Why TurboVec Fits SC Node

SC Node is a **local-first, privacy-first AI agent runtime**. Any memory / RAG layer must share these properties:

| Requirement | TurboVec Fit |
|-------------|--------------|
| **Local-first** | Pure Rust, no network, no external service. Runs entirely on-device. |
| **Rust-native** | Native crate, Cargo-managed. Fits SC Node's all-Rust stack. |
| **Compressed vectors** | 2-bit / 4-bit quantization examples -> 8-16x smaller than `f32` vectors. Critical for local CPU/edge. |
| **Fast SIMD search** | NEON, AVX-512BW, AVX2 fallback as described in README. Sub-millisecond ANN on CPU (conceptual). |
| **Filtered search** | Allowlist-based search / candidate-ID filtering. Metadata filters implemented by SC Node *above* TurboVec. |
| **Stable IDs** | `u64` keys stable across sessions. Enables incremental updates / deletes. |
| **Minimal external deps** | Only `half`, `bytemuck`, `memmap2`, `rayon`. Minimal audit surface. |
| **Embeddable** | Library, not service. Embeds directly in `sc-agent` binary. |

> **Note:** TurboVec API names below are based on the public README. The exact API surface must be verified against the crate before implementation.

---

## Future Module: `crates/sc-memory-turbovec`

```
crates/
  sc-memory-turbovec/
    src/
      lib.rs           # Public API: MemoryIndex, VectorEntry, SearchQuery
      index.rs         # TurboVec wrapper + mmap persistence
      embed.rs         # Embedding provider abstraction
      chunk.rs         # File chunking, metadata extraction
      search.rs        # Hybrid search (vector + filter)
    Cargo.toml
```

### Public API Sketch

```rust
pub struct MemoryIndex {
    // TurboVec index + mmap file
}

pub struct VectorEntry {
    pub id: u64,                    // Stable across sessions
    pub vector: Vec<f32>,           // Dense embedding (compressed internally)
    pub metadata: VectorMetadata,   // path, chunk_range, tags, timestamp
}

pub struct VectorMetadata {
    pub source_path: PathBuf,
    pub chunk_start: usize,
    pub chunk_end: usize,
    pub tags: Vec<String>,
    pub updated_at: DateTime<Utc>,
}

pub struct SearchQuery {
    pub vector: Vec<f32>,
    pub top_k: usize,
    pub filter: Option<MetadataFilter>,
}

pub enum MetadataFilter {
    PathPrefix(String),
    Tags(Vec<String>),
    TimestampRange { after: DateTime<Utc>, before: DateTime<Utc> },
    And(Box<MetadataFilter>, Box<MetadataFilter>),
    Or(Box<MetadataFilter>, Box<MetadataFilter>),
}

impl MemoryIndex {
    pub fn open(path: &Path) -> Result<Self>;
    pub fn upsert(&mut self, entries: &[VectorEntry]) -> Result<()>;
    pub fn delete(&mut self, ids: &[u64]) -> Result<()>;
    pub fn search(&self, query: SearchQuery) -> Result<Vec<ScoredEntry>>;
    pub fn flush(&mut self) -> Result<()>;
}
```

### TurboVec Integration Notes

- **Index type:** `turbovec::TurboQuantIndex` (based on README — verify exact path)
- **Stable IDs:** `turbovec::IdMapIndex` maps `u64` keys to internal indices (verify exact path)
- **Filtered search:** Implemented in SC Node *above* TurboVec:
  - Step 1: Apply metadata filters in SC Node -> produce candidate `u64` ID allowlist
  - Step 2: Pass allowlist to TurboVec search via candidate-ID filtering
  - Step 3: Post-process results (re-rank, format for agent)
- **Quantization:** 2-bit / 4-bit quantization examples (per README)
- **SIMD:** NEON, AVX-512BW, AVX2 fallback as described in README

---

## MVP Phases

### Phase 1 — Vector Index PoC
- Add `turbo` as optional workspace dependency (feature-gated)
- Implement `MemoryIndex::open()` with `mmap` + `turbovec::TurboQuantIndex`
- Basic CRUD: `upsert`, `delete`, `search` (k-NN only, no filters)
- Persistence: single-file `memory.bin` in `~/.sc-agent/memory/`
- Unit tests: insert 10k vectors, search k=10, verify recall

### Phase 2 — File Chunk Indexing
- `chunk.rs`: recursive walk of `workspace.allow` paths
- Chunking strategy: 512-token sliding window (overlap 50)
- Metadata: `source_path`, `chunk_start`, `chunk_end`, `tags` (ext, dir), `updated_at`
- Embedding via `sc-provider-ollama` (default) or `sc-provider-nvidia`
- Batch indexing with progress CLI: `sc-agent memory index`

### Phase 3 — Embedding Provider Abstraction
- New trait `EmbeddingProvider` in `sc-provider-core` (or `sc-memory-turbovec`)
- Implementations: `OllamaEmbedding`, `NvidiaEmbedding`, `OpenRouterEmbedding`
- Config: `[embedding]` section with `provider`, `model`, `batch_size`
- CLI: `sc-agent memory embed` (re-embed changed files)

### Phase 4 — Retrieval Injection into Agent Loop
- Extend `sc-agent-core` with `MemoryIndex` in `Session`
- New tool: `memory_search { query, top_k, filter }`
- Agent loop: on user query -> `memory_search` -> inject top-k chunks as context
- Config: `memory.enabled`, `memory.max_context_tokens`
- Evaluation: manual tests with codebase Q&A

---

## Exclusions (Not In Scope)

| Exclusion | Reason |
|-----------|--------|
| **Cloud vector DBs** (Pinecone, Weaviate, Qdrant Cloud) | Violates local-first, privacy-first. |
| **User data upload** | No telemetry, no data leaves device. |
| **Production promise** | This is an experimental alpha plan. No SLA, no guarantees. |
| **GPU acceleration** | TurboVec is CPU-first; GPU left for future. |
| **Cross-device sync** | Out of scope. Single-device memory. |
| **Multi-tenancy** | Single-user, single-device. |

---

## Note on current implementation

Since this plan was written, a backend-agnostic `sc-memory` crate was added to
the workspace (traits `MemoryStore` / `EmbeddingProvider`, a pure-Rust
`ReferenceMemoryStore`, and an optional `turbovec` feature). It is **not yet
wired into the runtime binary**. See [../STATUS.md](../STATUS.md).

---

## API Verification Needed Before Implementation

The following items are based on the public TurboVec README and must be verified against the actual crate API before writing any implementation code:

| Item | Assumed API | Verification Status |
|------|-------------|---------------------|
| Main index type | `turbovec::TurboQuantIndex` | Not verified |
| Stable ID mapping | `turbovec::IdMapIndex` | Not verified |
| Quantization config | 2-bit / 4-bit options | Not verified |
| SIMD support | NEON, AVX-512BW, AVX2 fallback | Not verified |
| mmap support | Feature flag `mmap` | Not verified |
| Candidate-ID filtering | Built-in or manual | Not verified |

## Decision Log

| Date | Decision |
|------|----------|
| 2026-07-09 | TurboVec selected as primary local vector index candidate. |
| 2026-07-10 | API assumptions corrected per README; verification section added. |
| 2026-07-10 | Unverified marketing claims removed; cautious wording applied. |
