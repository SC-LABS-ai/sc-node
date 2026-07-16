//! Verifiable execution proof bundles for SC Node.
//!
//! A [`ProofBundle`] is a machine-readable record of what happened during a
//! single agent task run: which git commits bounded it, what tool calls
//! were made and how they were decided (allow/ask/deny), what files
//! changed, which checks ran and whether they passed, and a tamper-evident
//! log of the audit events that occurred along the way.
//!
//! This crate is deliberately independent of `sc-contract`: `contract_hash`
//! is stored as an opaque string produced elsewhere (e.g.
//! `ExecutionContract::policy_hash`), not recomputed here. That keeps the
//! two crates decoupled and this one usable on its own.
//!
//! ## Determinism
//!
//! Nothing in this crate calls a clock or any other non-deterministic
//! source internally: timestamps ([`chrono::DateTime<Utc>`]) are always
//! accepted as constructor parameters, so building the exact same bundle
//! twice from the exact same inputs is reproducible and testable.
//!
//! ## Tamper evidence: v1 hash chain over audit events
//!
//! This crate implements the **hash-chain** option (rather than a single
//! whole-bundle digest) for tamper evidence, specifically over the
//! `audit_chain` field:
//!
//!   - Each [`AuditEvent`] is combined with the hash of the previous event
//!     ("genesis" is 32 zero bytes for the first event) via
//!     `SHA-256(prev_hash_bytes || canonical_json_bytes(event))`, producing
//!     a [`ChainedEvent`].
//!   - [`verify`] independently recomputes every link of the chain from the
//!     stored `event` payloads and compares against the stored `hash`
//!     values. Any modification to any event's payload - or to the stored
//!     hash of any link - changes a downstream hash and is detected.
//!   - This is a v1 design: it covers the ordered sequence of audit events,
//!     which is the part of a proof bundle most valuable to tamper with
//!     (e.g. to hide a denied action). Other bundle fields (changed files,
//!     check results, etc.) are not separately chained in v1; a future
//!     version could extend the chain to cover the whole bundle.
//!
//! ### Limitations: tamper-evident, not tamper-proof
//!
//! Be precise about what this chain does and does not guarantee:
//!
//!   - **Trailing truncation is undetected.** [`verify`] only re-derives
//!     the links that are actually present in `audit_chain`; dropping the
//!     last N events (and nothing else) yields a chain that still
//!     verifies, because there is no length/head anchor recorded
//!     independently of the chain itself. Use [`ProofBundle::chain_head`]
//!     (below) plus, where available, [`ProofBundle::with_expected_event_count`]
//!     and [`check_event_count`] to catch this class of tampering.
//!   - **A full re-hash is forgeable.** Anyone able to rewrite the whole
//!     bundle can recompute a self-consistent chain from scratch; there is
//!     no signature binding the chain to a trusted authority. The chain
//!     only proves internal self-consistency, not that the bundle reflects
//!     what actually happened. Callers that need that guarantee must
//!     externally anchor or sign [`ProofBundle::chain_head`] (e.g. commit
//!     it to an append-only log, or sign it with a key the chain itself
//!     has no access to).
//!   - **Non-chained fields have no integrity protection at all.**
//!     `denied_actions`, `checks`, `clippy_result`, `smoke_result`,
//!     `secret_scan_result`, `diff_summary`, and `changed_files` are plain
//!     data outside `audit_chain`; nothing in this crate detects
//!     modification of those fields. Only the audit events themselves are
//!     chained in v1.
//!
//! ## Redaction (best-effort, not exhaustive)
//!
//! [`AuditEvent::new`] passes its `data` payload through [`redact`] before
//! storing it. This is defense in depth, **not a guarantee that no secret
//! survives**:
//!
//!   - Any JSON object key whose *name* contains (case-insensitively) one
//!     of a small set of markers (`key`, `secret`, `token`, `password`,
//!     `credential`, `authorization`, `apikey`) has its *entire* value
//!     replaced with the literal string `"[REDACTED]"`, regardless of the
//!     value's shape. This is key-name-based and does not look at the
//!     value at all.
//!   - Independently, every JSON string *value* anywhere in the payload
//!     (an object value, a positional array element, or nested further)
//!     is scanned word-by-word for shapes that look like secret tokens:
//!     known vendor prefixes (`sk-`, `nvapi-`, `ghp_`/`gho_`/`ghu_`/`ghs_`/
//!     `ghr_`, `xoxb-`/`xoxp-`/`xoxa-`/`xoxr-`/`xoxs-`), the word
//!     immediately following a `Bearer` token, and any run of
//!     token-shaped characters (letters, digits, `-`, `_`, `+`, `/`, `=`)
//!     32 characters or longer (a conservative proxy for base64/hex
//!     secrets). Matching words are replaced with `[REDACTED]` in place;
//!     surrounding text is preserved.
//!
//! This scrubs the common shapes this crate was built to guard against -
//! `{"cmd": "curl -H 'Authorization: Bearer sk-xxx'"}` and
//! `{"args": ["--api-key", "sk-xxx"]}` are both redacted. It does **not**
//! guarantee every secret is caught: a short, opaque token with no
//! recognizable prefix embedded in ordinary-looking text can slip through.
//! Callers that construct [`AuditEvent`]s remain responsible for not
//! feeding genuinely sensitive raw values into `data` in the first place.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Genesis hash used as `prev_hash` for the first event in a chain: 32 zero
/// bytes. Not a real digest of anything; it just anchors the chain.
const GENESIS_HASH: [u8; 32] = [0u8; 32];

/// Key-name substrings (checked case-insensitively) that mark a JSON object
/// value as sensitive and subject to redaction.
const SENSITIVE_KEY_MARKERS: &[&str] = &[
    "key",
    "secret",
    "token",
    "password",
    "credential",
    "authorization",
    "apikey",
];

/// Known vendor secret-token prefixes (checked case-sensitively; these are
/// effectively never legitimate non-secret text). Covers `sk-`, `nvapi-`,
/// the GitHub `gh*_` family, and Slack's `xox[baprs]-` family.
const SECRET_VALUE_PREFIXES: &[&str] = &[
    "sk-", "nvapi-", "ghp_", "gho_", "ghu_", "ghs_", "ghr_", "xoxb-", "xoxp-", "xoxa-", "xoxr-",
    "xoxs-",
];

/// Minimum length of a contiguous run of token-shaped characters
/// (letters/digits/`-_+/=`) before it is treated as a likely
/// base64/hex-encoded secret and redacted, even without a recognized
/// vendor prefix. Conservative on purpose: false positives (redacting a
/// long non-secret identifier) are an acceptable cost; false negatives
/// (missing a real secret) are not.
const MIN_SECRET_RUN_LEN: usize = 32;

/// Whether `c` is a character that can appear inside a secret-token-shaped
/// run: alphanumeric, or one of the characters common to base64/hex/URL
/// encodings and CLI flag values.
fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '+' | '/' | '=')
}

/// Split a whitespace-delimited word into `(leading_punctuation, core,
/// trailing_punctuation)`, where `core` is the maximal run of
/// [`is_token_char`] characters. Used so punctuation immediately touching a
/// token (quotes, colons, trailing commas) is preserved unmodified when the
/// core is redacted.
fn split_core(word: &str) -> (&str, &str, &str) {
    let start = word.find(is_token_char).unwrap_or(word.len());
    let end = match word.rfind(is_token_char) {
        Some(i) => i + 1,
        None => start,
    };
    (&word[..start], &word[start..end], &word[end..])
}

/// Whether a token-shaped word (already isolated by [`split_core`]) looks
/// like a secret: it starts with a known vendor prefix, or it is a long
/// run of token characters (see [`MIN_SECRET_RUN_LEN`]).
fn looks_like_secret_token(core: &str) -> bool {
    if core.is_empty() {
        return false;
    }
    SECRET_VALUE_PREFIXES.iter().any(|p| core.starts_with(p)) || core.len() >= MIN_SECRET_RUN_LEN
}

/// Best-effort scrub of a free-text string value: walks the string
/// whitespace-delimited word by word (preserving all original whitespace
/// and adjacent punctuation exactly), and replaces a word's token-shaped
/// core with `[REDACTED]` when it looks like a secret (see
/// [`looks_like_secret_token`]) or when it immediately follows a literal
/// `Bearer` word (the standard HTTP bearer-token header shape), regardless
/// of whether that token itself matches a known prefix.
fn scrub_scalar_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_core_is_bearer = false;

    for chunk in s.split_inclusive(char::is_whitespace) {
        let ws_start = chunk
            .char_indices()
            .rev()
            .take_while(|&(_, c)| c.is_whitespace())
            .last()
            .map(|(i, _)| i);
        let (word, trailing_ws) = match ws_start {
            Some(i) => chunk.split_at(i),
            None => (chunk, ""),
        };

        let (prefix, core, suffix) = split_core(word);
        let is_bearer_token = prev_core_is_bearer && !core.is_empty();
        if is_bearer_token || looks_like_secret_token(core) {
            out.push_str(prefix);
            out.push_str("[REDACTED]");
            out.push_str(suffix);
        } else {
            out.push_str(word);
        }
        out.push_str(trailing_ws);

        prev_core_is_bearer = core.eq_ignore_ascii_case("bearer");
    }

    out
}

/// Errors produced while building, serializing, or verifying a proof bundle.
#[derive(Debug, Error)]
pub enum ProofError {
    #[error("failed to serialize proof data: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("audit hash chain broken at event index {index}: expected {expected}, found {found}")]
    ChainBroken {
        index: usize,
        expected: String,
        found: String,
    },
    #[error("audit chain event count mismatch: expected {expected}, found {actual}")]
    EventCountMismatch { expected: u32, actual: u32 },
}

/// Recursively redact sensitive values from a JSON value.
///
/// Any object key whose name contains (case-insensitively) one of
/// [`SENSITIVE_KEY_MARKERS`] has its value replaced with
/// `"[REDACTED]"`; nested objects/arrays are otherwise redacted
/// recursively so secrets cannot hide a level deeper. Independently, every
/// string value (object value, array element, or otherwise) is passed
/// through [`scrub_scalar_string`], so secret-shaped tokens are redacted
/// even when they are not behind a sensitively-named key (e.g. a
/// positional CLI argument or an embedded `Authorization: Bearer ...`
/// header inside a larger command string). See the module docs for the
/// exact rules and their limits.
pub fn redact(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, val) in map {
                let lower = key.to_lowercase();
                if SENSITIVE_KEY_MARKERS
                    .iter()
                    .any(|marker| lower.contains(marker))
                {
                    out.insert(
                        key.clone(),
                        serde_json::Value::String("[REDACTED]".to_string()),
                    );
                } else {
                    out.insert(key.clone(), redact(val));
                }
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(redact).collect())
        }
        serde_json::Value::String(s) => serde_json::Value::String(scrub_scalar_string(s)),
        other => other.clone(),
    }
}

/// Canonical JSON bytes for any serializable value: sorted object keys
/// (via `serde_json::Value`'s `BTreeMap`-backed `Map`, since this workspace
/// never enables `preserve_order`), no incidental whitespace.
fn canonical_bytes_of<T: Serialize>(value: &T) -> Result<Vec<u8>, ProofError> {
    let as_value = serde_json::to_value(value)?;
    Ok(serde_json::to_vec(&as_value)?)
}

/// A single audit event recorded during task execution.
///
/// Construct via [`AuditEvent::new`], which redacts `data` before storing
/// it - never build one with un-redacted sensitive data directly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditEvent {
    pub seq: u32,
    pub timestamp: DateTime<Utc>,
    pub kind: String,
    pub description: String,
    pub data: serde_json::Value,
}

impl AuditEvent {
    /// Create a new audit event, redacting `data` before storing it.
    pub fn new(
        seq: u32,
        timestamp: DateTime<Utc>,
        kind: impl Into<String>,
        description: impl Into<String>,
        data: serde_json::Value,
    ) -> Self {
        Self {
            seq,
            timestamp,
            kind: kind.into(),
            description: description.into(),
            data: redact(&data),
        }
    }
}

/// An [`AuditEvent`] together with the hash that chains it to its
/// predecessor. See the module docs for the exact hash construction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChainedEvent {
    pub event: AuditEvent,
    /// Hex-encoded SHA-256 of `prev_hash_bytes || canonical_json_bytes(event)`.
    pub hash: String,
}

fn hash_step(prev: &[u8; 32], event: &AuditEvent) -> Result<[u8; 32], ProofError> {
    let event_bytes = canonical_bytes_of(event)?;
    let mut hasher = Sha256::new();
    hasher.update(prev);
    hasher.update(&event_bytes);
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    Ok(out)
}

/// Build a tamper-evident hash chain over an ordered sequence of audit
/// events. Pure function of its input: the same events in the same order
/// always produce the same chain.
pub fn build_chain(events: Vec<AuditEvent>) -> Result<Vec<ChainedEvent>, ProofError> {
    let mut prev = GENESIS_HASH;
    let mut chain = Vec::with_capacity(events.len());
    for event in events {
        let digest = hash_step(&prev, &event)?;
        chain.push(ChainedEvent {
            event,
            hash: hex::encode(digest),
        });
        prev = digest;
    }
    Ok(chain)
}

/// Independently recompute an audit hash chain and compare it against the
/// stored hashes. Returns `Ok(())` if every link matches, otherwise the
/// first mismatch found.
pub fn verify(bundle: &ProofBundle) -> Result<(), ProofError> {
    let mut prev = GENESIS_HASH;
    for (index, chained) in bundle.audit_chain.iter().enumerate() {
        let digest = hash_step(&prev, &chained.event)?;
        let expected = hex::encode(digest);
        if expected != chained.hash {
            return Err(ProofError::ChainBroken {
                index,
                expected,
                found: chained.hash.clone(),
            });
        }
        prev = digest;
    }
    Ok(())
}

/// The hash of the most recent link in an audit chain (the "chain head"):
/// the value a caller should externally anchor (sign it, commit it to an
/// append-only log, etc.) since the chain by itself carries no signature
/// and a full rewrite is otherwise undetectable (see the module docs).
/// `None` for an empty chain.
pub fn chain_head(chain: &[ChainedEvent]) -> Option<&str> {
    chain.last().map(|c| c.hash.as_str())
}

/// Compare the number of events actually present in `bundle.audit_chain`
/// against an independently recorded expected count (if the bundle has
/// one; see [`ProofBundle::with_expected_event_count`]). This exists
/// because the hash chain alone cannot detect trailing truncation: an
/// expected count recorded separately from the chain (e.g. incremented by
/// the caller as events occur) is one cheap way to catch it.
///
/// Returns `Ok(())` if the bundle has no expected count recorded, or if
/// the counts match; otherwise returns the mismatch.
pub fn check_event_count(bundle: &ProofBundle) -> Result<(), ProofError> {
    if let Some(expected) = bundle.expected_event_count {
        let actual = bundle.audit_chain.len() as u32;
        if actual != expected {
            return Err(ProofError::EventCountMismatch { expected, actual });
        }
    }
    Ok(())
}

/// Aggregate tool-call counts, broken down by decision.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallTotals {
    pub total: u32,
    pub allowed: u32,
    pub asked: u32,
    pub denied: u32,
}

/// A summary of one kind of denied action, with an occurrence count.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeniedAction {
    pub tool: String,
    pub reason: String,
    pub count: u32,
}

/// Summary of the file diff produced by the task.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffSummary {
    pub files_changed: u32,
    pub insertions: u32,
    pub deletions: u32,
}

/// The outcome of running a single named command/check (e.g. `cargo test`,
/// clippy, a smoke test, or a secret scan).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckOutcome {
    pub name: String,
    pub command: String,
    pub passed: bool,
    pub summary: String,
}

impl CheckOutcome {
    pub fn new(
        name: impl Into<String>,
        command: impl Into<String>,
        passed: bool,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            command: command.into(),
            passed,
            summary: summary.into(),
        }
    }

    /// Placeholder outcome for a check that was not (yet) run.
    pub fn not_run(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            command: String::new(),
            passed: false,
            summary: "not run".to_string(),
        }
    }
}

/// A verifiable, machine-readable record of a single task execution.
///
/// Build one with [`ProofBundle::new`] and the `with_*` builder methods.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProofBundle {
    pub task_id: String,
    /// Opaque hash of the [`ExecutionContract`](../sc_contract/struct.ExecutionContract.html)
    /// this run was governed by (see that crate's `policy_hash`).
    pub contract_hash: String,
    pub start_commit: String,
    pub end_commit: String,
    pub start_dirty: bool,
    pub end_dirty: bool,
    pub worker: String,
    pub provider: String,
    pub model: String,
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
    pub tool_call_totals: ToolCallTotals,
    pub denied_actions: Vec<DeniedAction>,
    pub changed_files: Vec<String>,
    pub diff_summary: DiffSummary,
    pub checks: Vec<CheckOutcome>,
    pub clippy_result: CheckOutcome,
    pub smoke_result: CheckOutcome,
    pub secret_scan_result: CheckOutcome,
    /// Sorted map of artifact name/path to a hex-encoded content hash.
    pub artifact_hashes: BTreeMap<String, String>,
    pub known_limitations: Vec<String>,
    /// Tamper-evident hash chain over the events observed during
    /// execution. See the module docs for the chain construction.
    pub audit_chain: Vec<ChainedEvent>,
    /// Optional, independently-recorded expected count of audit events
    /// (e.g. incremented by the caller as events occur, separately from
    /// `audit_chain` itself). Used by [`check_event_count`] to catch
    /// trailing truncation, which the hash chain alone cannot detect.
    pub expected_event_count: Option<u32>,
}

impl ProofBundle {
    /// Create a new bundle with its required identity/timing fields.
    /// Everything else starts at a safe empty/not-run default and is filled
    /// in via the `with_*` builder methods.
    pub fn new(
        task_id: impl Into<String>,
        contract_hash: impl Into<String>,
        worker: impl Into<String>,
        provider: impl Into<String>,
        model: impl Into<String>,
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
    ) -> Self {
        Self {
            task_id: task_id.into(),
            contract_hash: contract_hash.into(),
            start_commit: String::new(),
            end_commit: String::new(),
            start_dirty: false,
            end_dirty: false,
            worker: worker.into(),
            provider: provider.into(),
            model: model.into(),
            start_time,
            end_time,
            tool_call_totals: ToolCallTotals::default(),
            denied_actions: Vec::new(),
            changed_files: Vec::new(),
            diff_summary: DiffSummary::default(),
            checks: Vec::new(),
            clippy_result: CheckOutcome::not_run("clippy"),
            smoke_result: CheckOutcome::not_run("smoke"),
            secret_scan_result: CheckOutcome::not_run("secret_scan"),
            artifact_hashes: BTreeMap::new(),
            known_limitations: Vec::new(),
            audit_chain: Vec::new(),
            expected_event_count: None,
        }
    }

    pub fn with_commits(
        mut self,
        start_commit: impl Into<String>,
        end_commit: impl Into<String>,
        start_dirty: bool,
        end_dirty: bool,
    ) -> Self {
        self.start_commit = start_commit.into();
        self.end_commit = end_commit.into();
        self.start_dirty = start_dirty;
        self.end_dirty = end_dirty;
        self
    }

    pub fn with_tool_call_totals(mut self, totals: ToolCallTotals) -> Self {
        self.tool_call_totals = totals;
        self
    }

    pub fn with_denied_actions(mut self, denied_actions: Vec<DeniedAction>) -> Self {
        self.denied_actions = denied_actions;
        self
    }

    pub fn with_changed_files(mut self, changed_files: Vec<String>) -> Self {
        self.changed_files = changed_files;
        self
    }

    pub fn with_diff_summary(mut self, diff_summary: DiffSummary) -> Self {
        self.diff_summary = diff_summary;
        self
    }

    pub fn with_checks(mut self, checks: Vec<CheckOutcome>) -> Self {
        self.checks = checks;
        self
    }

    pub fn with_clippy_result(mut self, outcome: CheckOutcome) -> Self {
        self.clippy_result = outcome;
        self
    }

    pub fn with_smoke_result(mut self, outcome: CheckOutcome) -> Self {
        self.smoke_result = outcome;
        self
    }

    pub fn with_secret_scan_result(mut self, outcome: CheckOutcome) -> Self {
        self.secret_scan_result = outcome;
        self
    }

    pub fn with_artifact_hashes(mut self, artifact_hashes: BTreeMap<String, String>) -> Self {
        self.artifact_hashes = artifact_hashes;
        self
    }

    pub fn with_known_limitations(mut self, known_limitations: Vec<String>) -> Self {
        self.known_limitations = known_limitations;
        self
    }

    /// Build the tamper-evident chain from an ordered sequence of audit
    /// events and attach it to this bundle.
    pub fn with_audit_events(mut self, events: Vec<AuditEvent>) -> Result<Self, ProofError> {
        self.audit_chain = build_chain(events)?;
        Ok(self)
    }

    /// Record an independently-tracked expected event count, used by
    /// [`check_event_count`] to catch trailing truncation of `audit_chain`
    /// (see the module docs on the limits of the hash chain alone).
    pub fn with_expected_event_count(mut self, expected: u32) -> Self {
        self.expected_event_count = Some(expected);
        self
    }

    /// The hash of the most recent audit-chain link (the "chain head").
    /// This is the value a caller should externally anchor (sign it,
    /// commit it to an append-only log, etc.), since the chain itself
    /// carries no signature. `None` if there are no audit events.
    pub fn chain_head(&self) -> Option<&str> {
        chain_head(&self.audit_chain)
    }

    /// Canonical JSON representation of this bundle: sorted object keys, no
    /// incidental whitespace. See the crate docs for why this is
    /// deterministic in this workspace.
    pub fn canonical_json(&self) -> Result<String, ProofError> {
        let bytes = canonical_bytes_of(self)?;
        Ok(String::from_utf8(bytes).expect("serde_json output is always valid UTF-8"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(seconds, 0).expect("valid timestamp")
    }

    fn sample_events() -> Vec<AuditEvent> {
        vec![
            AuditEvent::new(
                0,
                ts(1_700_000_000),
                "tool_call",
                "file_write allowed",
                serde_json::json!({"tool": "file_write", "path": "/workspace/example/src/lib.rs"}),
            ),
            AuditEvent::new(
                1,
                ts(1_700_000_010),
                "tool_call",
                "shell denied",
                serde_json::json!({"tool": "shell", "cmd": "rm -rf /"}),
            ),
        ]
    }

    fn sample_bundle() -> ProofBundle {
        ProofBundle::new(
            "task-001",
            "deadbeef",
            "worker-a",
            "ollama",
            "local-model-a",
            ts(1_700_000_000),
            ts(1_700_000_100),
        )
        .with_commits("abc123", "def456", false, true)
        .with_tool_call_totals(ToolCallTotals {
            total: 2,
            allowed: 1,
            asked: 0,
            denied: 1,
        })
        .with_denied_actions(vec![DeniedAction {
            tool: "shell".to_string(),
            reason: "deny_pattern".to_string(),
            count: 1,
        }])
        .with_changed_files(vec!["src/lib.rs".to_string()])
        .with_diff_summary(DiffSummary {
            files_changed: 1,
            insertions: 10,
            deletions: 2,
        })
        .with_checks(vec![CheckOutcome::new(
            "cargo test",
            "cargo test --workspace",
            true,
            "all tests passed",
        )])
        .with_clippy_result(CheckOutcome::new(
            "clippy",
            "cargo clippy",
            true,
            "no warnings",
        ))
        .with_smoke_result(CheckOutcome::new(
            "smoke",
            "sc-agent --version",
            true,
            "ran",
        ))
        .with_secret_scan_result(CheckOutcome::new("secret_scan", "scan", true, "clean"))
        .with_artifact_hashes(BTreeMap::from([(
            "target/release/sc-agent".to_string(),
            "0123456789abcdef".to_string(),
        )]))
        .with_known_limitations(vec!["no windows sandboxing yet".to_string()])
        .with_audit_events(sample_events())
        .expect("chain build should succeed")
    }

    #[test]
    fn build_and_verify_passes() {
        let bundle = sample_bundle();
        // canonical serialization must succeed and be stable
        let json_a = bundle.canonical_json().unwrap();
        let json_b = bundle.canonical_json().unwrap();
        assert_eq!(json_a, json_b);
        assert!(verify(&bundle).is_ok());
    }

    #[test]
    fn tamper_with_event_breaks_verification() {
        let mut bundle = sample_bundle();
        bundle.audit_chain[0].event.description = "tampered".to_string();
        let result = verify(&bundle);
        assert!(matches!(
            result,
            Err(ProofError::ChainBroken { index: 0, .. })
        ));
    }

    #[test]
    fn tamper_with_stored_hash_breaks_verification() {
        let mut bundle = sample_bundle();
        bundle.audit_chain[1].hash = "0".repeat(64);
        let result = verify(&bundle);
        assert!(matches!(
            result,
            Err(ProofError::ChainBroken { index: 1, .. })
        ));
    }

    #[test]
    fn redaction_hides_secret_from_serialized_bundle() {
        let secret_event = AuditEvent::new(
            0,
            ts(1_700_000_000),
            "tool_call",
            "provider call",
            serde_json::json!({"api_key": "sk-super-secret-value", "model": "local-model-a"}),
        );
        let bundle = ProofBundle::new(
            "task-002",
            "deadbeef",
            "worker-a",
            "openrouter",
            "some-model",
            ts(0),
            ts(1),
        )
        .with_audit_events(vec![secret_event])
        .unwrap();

        let json = bundle.canonical_json().unwrap();
        assert!(!json.contains("sk-super-secret-value"));
        assert!(json.contains("[REDACTED]"));
        // Non-sensitive sibling field must still be present.
        assert!(json.contains("some-model") || json.contains("local-model-a"));
    }

    #[test]
    fn redact_recurses_into_nested_objects_and_arrays() {
        let value = serde_json::json!({
            "outer": {
                "nested_secret_token": "abc",
                "list": [{"password": "xyz"}, {"safe": "value"}]
            }
        });
        let redacted = redact(&value);
        let text = redacted.to_string();
        assert!(!text.contains("abc"));
        assert!(!text.contains("xyz"));
        assert!(text.contains("safe"));
        assert!(text.contains("value"));
    }

    #[test]
    fn hash_chain_links_correctly() {
        let events = sample_events();
        let chain = build_chain(events.clone()).unwrap();
        assert_eq!(chain.len(), 2);

        // Manually recompute the first link from genesis.
        let first_expected = hash_step(&GENESIS_HASH, &chain[0].event).unwrap();
        assert_eq!(chain[0].hash, hex::encode(first_expected));

        // Second link must depend on the first link's hash, not genesis.
        let second_expected = hash_step(&first_expected, &chain[1].event).unwrap();
        assert_eq!(chain[1].hash, hex::encode(second_expected));

        // Sanity: chaining from genesis directly for the second event must
        // NOT match (proves the chain actually depends on the previous hash).
        let wrong = hash_step(&GENESIS_HASH, &chain[1].event).unwrap();
        assert_ne!(chain[1].hash, hex::encode(wrong));
    }

    #[test]
    fn empty_chain_verifies_trivially() {
        let bundle = ProofBundle::new(
            "task-003",
            "deadbeef",
            "worker-a",
            "ollama",
            "m",
            ts(0),
            ts(1),
        );
        assert!(verify(&bundle).is_ok());
    }

    #[test]
    fn redaction_scrubs_secret_embedded_in_command_string_value() {
        // Key is "cmd" (not a sensitive key name), so this only gets
        // caught by the value-side scrub, not the key-name rule.
        let event = AuditEvent::new(
            0,
            ts(0),
            "tool_call",
            "shell command",
            serde_json::json!({
                "cmd": "curl -H 'Authorization: Bearer sk-fake1234567890abcd' https://example.com"
            }),
        );
        let bundle = ProofBundle::new(
            "task-004",
            "deadbeef",
            "worker-a",
            "openrouter",
            "m",
            ts(0),
            ts(1),
        )
        .with_audit_events(vec![event])
        .unwrap();

        let json = bundle.canonical_json().unwrap();
        assert!(!json.contains("sk-fake1234567890abcd"));
        assert!(json.contains("[REDACTED]"));
        // Surrounding, non-secret text must be preserved.
        assert!(json.contains("curl"));
        assert!(json.contains("Bearer"));
    }

    #[test]
    fn redaction_scrubs_secret_in_positional_array_element() {
        // Key is "args" (not sensitive); the secret is a bare positional
        // array element with no key name at all to hang a rule off of.
        let event = AuditEvent::new(
            0,
            ts(0),
            "tool_call",
            "provider call",
            serde_json::json!({"args": ["--api-key", "sk-fake1234567890abcd"]}),
        );
        let bundle = ProofBundle::new(
            "task-005",
            "deadbeef",
            "worker-a",
            "openrouter",
            "m",
            ts(0),
            ts(1),
        )
        .with_audit_events(vec![event])
        .unwrap();

        let json = bundle.canonical_json().unwrap();
        assert!(!json.contains("sk-fake1234567890abcd"));
        assert!(json.contains("[REDACTED]"));
        // The flag name itself (not a secret) must be preserved.
        assert!(json.contains("--api-key"));
    }

    #[test]
    fn chain_head_exposes_last_link_hash() {
        let bundle = sample_bundle();
        let expected = bundle.audit_chain.last().unwrap().hash.clone();
        assert_eq!(bundle.chain_head(), Some(expected.as_str()));
        assert_eq!(chain_head(&bundle.audit_chain), Some(expected.as_str()));
    }

    #[test]
    fn chain_head_is_none_for_empty_chain() {
        let bundle = ProofBundle::new(
            "task-006",
            "deadbeef",
            "worker-a",
            "ollama",
            "m",
            ts(0),
            ts(1),
        );
        assert_eq!(bundle.chain_head(), None);
    }

    #[test]
    fn check_event_count_detects_truncation() {
        let mut bundle = sample_bundle().with_expected_event_count(2);
        assert!(check_event_count(&bundle).is_ok());

        // Simulate a trailing-truncation attack: drop the last event from
        // the chain without updating the independently-recorded expected
        // count.
        bundle.audit_chain.pop();
        let result = check_event_count(&bundle);
        assert!(matches!(
            result,
            Err(ProofError::EventCountMismatch {
                expected: 2,
                actual: 1
            })
        ));
        // The hash chain itself still verifies: this is exactly the class
        // of tampering it cannot detect on its own (see module docs).
        assert!(verify(&bundle).is_ok());
    }
}
