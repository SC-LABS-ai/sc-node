//! Versioned execution contracts for SC Node.
//!
//! An [`ExecutionContract`] is a security policy document that governs a
//! single agent task: which tools/paths/network hosts/providers/models it
//! may use, whether it may commit or push, and what limits and approvals
//! apply. Contracts are parsed strictly (unknown fields are rejected) and
//! **fail closed**: any security-critical field that is absent from the
//! input defaults to the most restrictive possible value rather than a
//! permissive one. See [`ExecutionContract::parse`] and the field-level
//! docs on the policy enums below for the exact fail-closed defaults.
//!
//! ## Wire format: TOML, not JSON
//!
//! Contracts are authored and parsed as **TOML**, not JSON:
//!   - It is the format already used for all other SC Node configuration
//!     (see the `sc-config` crate), so operators only need to learn one
//!     syntax.
//!   - TOML supports comments, which matters for a security artifact that
//!     humans author and review (operators can annotate *why* a policy was
//!     chosen).
//!   - The `toml` crate is already a pinned workspace dependency; no new
//!     dependency is required.
//!
//! Canonicalization and hashing (see [`ExecutionContract::canonical_json`])
//! always go through JSON internally, independent of the wire format: JSON
//! gives a trivially deterministic byte representation once object keys are
//! sorted, and `serde_json`'s `Map` type is backed by a `BTreeMap` in this
//! workspace (the `preserve_order` feature is not enabled anywhere), so
//! round-tripping a value through `serde_json::Value` sorts every object's
//! keys automatically.

pub mod preflight;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Only schema version currently understood by this crate.
///
/// A contract whose `schema_version` does not match this value is rejected
/// by [`ExecutionContract::validate`] rather than silently interpreted under
/// possibly-different semantics.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

const DEFAULT_MAX_TOOL_ROUNDS: u32 = 20;
const DEFAULT_MAX_RUNTIME_SECS: u64 = 600;
const DEFAULT_MAX_FILES_CHANGED: u32 = 20;
const DEFAULT_MAX_OUTPUT_BYTES: u64 = 1_000_000;

/// Errors produced while parsing, validating, or serializing a contract.
#[derive(Debug, Error)]
pub enum ContractError {
    #[error("failed to parse contract: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("failed to serialize contract: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("invalid contract: {0}")]
    Validation(String),
}

/// Network access policy for the executing task.
///
/// Defaults to [`NetworkPolicy::Deny`] (fail closed) when absent from the
/// input: a contract that does not say anything about network access must
/// not be interpreted as permitting it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum NetworkPolicy {
    /// No network access permitted. Most restrictive; the default.
    #[default]
    Deny,
    /// Network access permitted only to the listed hosts.
    AllowList { hosts: Vec<String> },
    /// Unrestricted network access. Must be set explicitly; never a default.
    Allow,
}

impl NetworkPolicy {
    pub fn describe(&self) -> String {
        match self {
            NetworkPolicy::Deny => "deny (no network access)".to_string(),
            NetworkPolicy::AllowList { hosts } => {
                format!("allow_list ({} host(s))", hosts.len())
            }
            NetworkPolicy::Allow => "allow (unrestricted)".to_string(),
        }
    }
}

/// Which model providers the executing task may talk to.
///
/// Defaults to [`ProviderPolicy::LocalOnly`] (fail closed) when absent: a
/// contract silent on provider policy must not be interpreted as permitting
/// arbitrary cloud providers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ProviderPolicy {
    /// Only local providers (e.g. `ollama`, `local`). Most restrictive; the default.
    #[default]
    LocalOnly,
    /// Only the listed providers.
    AllowList { providers: Vec<String> },
    /// Any provider. Must be set explicitly; never a default.
    Any,
}

impl ProviderPolicy {
    pub fn describe(&self) -> String {
        match self {
            ProviderPolicy::LocalOnly => "local_only".to_string(),
            ProviderPolicy::AllowList { providers } => {
                format!("allow_list ({} provider(s))", providers.len())
            }
            ProviderPolicy::Any => "any".to_string(),
        }
    }
}

/// Which models the executing task may request.
///
/// Defaults to an *empty* [`ModelPolicy::AllowList`] (fail closed) when
/// absent: with no models explicitly permitted, none may be used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelPolicy {
    /// Only the listed models. An empty list allows nothing (the default).
    AllowList { models: Vec<String> },
    /// Any model. Must be set explicitly; never a default.
    Any,
}

impl Default for ModelPolicy {
    fn default() -> Self {
        ModelPolicy::AllowList { models: Vec::new() }
    }
}

impl ModelPolicy {
    pub fn describe(&self) -> String {
        match self {
            ModelPolicy::AllowList { models } => {
                format!("allow_list ({} model(s))", models.len())
            }
            ModelPolicy::Any => "any".to_string(),
        }
    }
}

/// Whether task data may leave the local machine.
///
/// Defaults to [`DataBoundary::LocalOnly`] (fail closed) when absent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataBoundary {
    /// Data must never leave the local machine. Most restrictive; the default.
    #[default]
    LocalOnly,
    /// Data may be sent to external/cloud providers.
    CloudAllowed,
}

impl DataBoundary {
    pub fn describe(&self) -> &'static str {
        match self {
            DataBoundary::LocalOnly => "local_only",
            DataBoundary::CloudAllowed => "cloud_allowed",
        }
    }
}

/// Whether the executing task may create git commits.
///
/// Defaults to [`CommitPolicy::Never`] (fail closed) when absent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitPolicy {
    /// No commits may be created. Most restrictive; the default.
    #[default]
    Never,
    /// Commits require explicit human approval, one at a time.
    ApprovalRequired,
    /// The task may commit autonomously within the contract's other limits.
    Auto,
}

impl CommitPolicy {
    pub fn describe(&self) -> &'static str {
        match self {
            CommitPolicy::Never => "never",
            CommitPolicy::ApprovalRequired => "approval_required",
            CommitPolicy::Auto => "auto",
        }
    }
}

/// Whether the executing task may push to a remote.
///
/// Defaults to [`PushPolicy::Never`] (fail closed) when absent. This matches
/// SC Node's baseline "no push" posture: pushing must always be an explicit,
/// deliberate opt-in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PushPolicy {
    /// The task may never push. Most restrictive; the default.
    #[default]
    Never,
    /// Pushing requires explicit human approval, one at a time.
    ApprovalRequired,
    /// The task may push autonomously within the contract's other limits.
    Auto,
}

impl PushPolicy {
    pub fn describe(&self) -> &'static str {
        match self {
            PushPolicy::Never => "never",
            PushPolicy::ApprovalRequired => "approval_required",
            PushPolicy::Auto => "auto",
        }
    }
}

/// Which actions require a human approval before proceeding.
///
/// Defaults to [`ApprovalScope::All`] (fail closed) when absent: with no
/// approval scope specified, every action requires approval rather than
/// none.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ApprovalScope {
    /// Every action requires approval. Most restrictive; the default.
    #[default]
    All,
    /// Only the listed action categories require approval (e.g. `"push"`,
    /// `"commit"`, `"network"`).
    List(Vec<String>),
    /// No action requires approval. Must be set explicitly; never a default.
    None,
}

impl ApprovalScope {
    pub fn describe(&self) -> String {
        match self {
            ApprovalScope::All => "all".to_string(),
            ApprovalScope::List(items) => format!("list ({} item(s))", items.len()),
            ApprovalScope::None => "none".to_string(),
        }
    }
}

/// Audit logging policy.
///
/// Defaults to logging everything (fail closed / safe default: more
/// oversight, not less).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AuditPolicy {
    pub log_tool_calls: bool,
    pub log_args: bool,
    pub log_output: bool,
}

impl Default for AuditPolicy {
    fn default() -> Self {
        Self {
            log_tool_calls: true,
            log_args: true,
            log_output: true,
        }
    }
}

/// Default denied path patterns applied when `denied_paths` is absent from
/// the input. These are additional defense-in-depth denials on top of the
/// (fail-closed, empty-by-default) `allowed_paths` allowlist; they mirror
/// the sensitive-path defaults already used by `sc-config`'s
/// `WorkspaceConfig`.
fn default_denied_paths() -> Vec<String> {
    vec![
        "~/.ssh".to_string(),
        "~/.aws".to_string(),
        "~/.gnupg".to_string(),
        "**/.git/**".to_string(),
        "**/.env*".to_string(),
        "**/node_modules/**".to_string(),
        "**/target/**".to_string(),
    ]
}

/// Wire-format representation of a contract, used only during parsing.
///
/// Every security-critical field is `Option` here so that "absent from the
/// input" can be distinguished from "explicitly present" and mapped to a
/// fail-closed default in [`RawExecutionContract::into_contract`]. Fields
/// with no safe implicit meaning (`task_id`, `task`, `worker`, `workspace`,
/// `schema_version`) are *not* optional: a contract missing them is simply
/// invalid and parsing fails, rather than guessing a value.
///
/// `deny_unknown_fields` makes unrecognized keys a hard parse error, so a
/// typo'd or forward-incompatible field can never be silently ignored.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawExecutionContract {
    schema_version: u32,
    task_id: String,
    task: String,
    worker: String,
    workspace: String,
    #[serde(default)]
    allowed_tools: Option<Vec<String>>,
    #[serde(default)]
    denied_tools: Option<Vec<String>>,
    #[serde(default)]
    allowed_paths: Option<Vec<String>>,
    #[serde(default)]
    denied_paths: Option<Vec<String>>,
    #[serde(default)]
    network_policy: Option<NetworkPolicy>,
    #[serde(default)]
    provider_policy: Option<ProviderPolicy>,
    #[serde(default)]
    model_policy: Option<ModelPolicy>,
    #[serde(default)]
    data_boundary: Option<DataBoundary>,
    #[serde(default)]
    max_tool_rounds: Option<u32>,
    #[serde(default)]
    max_runtime: Option<u64>,
    #[serde(default)]
    max_files_changed: Option<u32>,
    #[serde(default)]
    max_output_bytes: Option<u64>,
    #[serde(default)]
    approvals_required: Option<ApprovalScope>,
    #[serde(default)]
    commit_policy: Option<CommitPolicy>,
    #[serde(default)]
    push_policy: Option<PushPolicy>,
    #[serde(default)]
    audit_policy: Option<AuditPolicy>,
}

impl RawExecutionContract {
    fn into_contract(self) -> ExecutionContract {
        ExecutionContract {
            schema_version: self.schema_version,
            task_id: self.task_id,
            task: self.task,
            worker: self.worker,
            workspace: self.workspace,
            // Fail closed: absent allowlists mean nothing is allowed.
            allowed_tools: self.allowed_tools.unwrap_or_default(),
            denied_tools: self.denied_tools.unwrap_or_default(),
            allowed_paths: self.allowed_paths.unwrap_or_default(),
            denied_paths: self.denied_paths.unwrap_or_else(default_denied_paths),
            network_policy: self.network_policy.unwrap_or_default(),
            provider_policy: self.provider_policy.unwrap_or_default(),
            model_policy: self.model_policy.unwrap_or_default(),
            data_boundary: self.data_boundary.unwrap_or_default(),
            max_tool_rounds: self.max_tool_rounds.unwrap_or(DEFAULT_MAX_TOOL_ROUNDS),
            max_runtime: self.max_runtime.unwrap_or(DEFAULT_MAX_RUNTIME_SECS),
            max_files_changed: self.max_files_changed.unwrap_or(DEFAULT_MAX_FILES_CHANGED),
            max_output_bytes: self.max_output_bytes.unwrap_or(DEFAULT_MAX_OUTPUT_BYTES),
            approvals_required: self.approvals_required.unwrap_or_default(),
            commit_policy: self.commit_policy.unwrap_or_default(),
            push_policy: self.push_policy.unwrap_or_default(),
            audit_policy: self.audit_policy.unwrap_or_default(),
        }
    }
}

/// A fully-normalized, validated execution contract.
///
/// Every field is populated (either from the input or from a fail-closed
/// default); there is no further ambiguity between "unspecified" and
/// "explicitly restrictive" once a value of this type exists.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionContract {
    pub schema_version: u32,
    pub task_id: String,
    pub task: String,
    pub worker: String,
    pub workspace: String,
    pub allowed_tools: Vec<String>,
    pub denied_tools: Vec<String>,
    pub allowed_paths: Vec<String>,
    pub denied_paths: Vec<String>,
    pub network_policy: NetworkPolicy,
    pub provider_policy: ProviderPolicy,
    pub model_policy: ModelPolicy,
    pub data_boundary: DataBoundary,
    pub max_tool_rounds: u32,
    /// Wall-clock execution limit, in seconds.
    pub max_runtime: u64,
    pub max_files_changed: u32,
    pub max_output_bytes: u64,
    pub approvals_required: ApprovalScope,
    pub commit_policy: CommitPolicy,
    pub push_policy: PushPolicy,
    pub audit_policy: AuditPolicy,
}

impl ExecutionContract {
    /// Parse an execution contract from TOML source text.
    ///
    /// Unknown fields anywhere in the document are rejected. Absent
    /// security-critical fields are filled in with fail-closed defaults
    /// (see the field docs on [`NetworkPolicy`], [`ProviderPolicy`],
    /// [`ModelPolicy`], [`DataBoundary`], [`CommitPolicy`], [`PushPolicy`],
    /// and [`ApprovalScope`]). The result is validated before being
    /// returned.
    pub fn parse(input: &str) -> Result<Self, ContractError> {
        let raw: RawExecutionContract = toml::from_str(input)?;
        let contract = raw.into_contract();
        contract.validate()?;
        Ok(contract)
    }

    /// Validate structural/semantic invariants that are not expressible
    /// through fail-closed field defaults alone.
    pub fn validate(&self) -> Result<(), ContractError> {
        if self.schema_version != CURRENT_SCHEMA_VERSION {
            return Err(ContractError::Validation(format!(
                "unsupported schema_version {} (expected {})",
                self.schema_version, CURRENT_SCHEMA_VERSION
            )));
        }
        if self.task_id.trim().is_empty() {
            return Err(ContractError::Validation("task_id cannot be empty".into()));
        }
        if self.task.trim().is_empty() {
            return Err(ContractError::Validation("task cannot be empty".into()));
        }
        if self.worker.trim().is_empty() {
            return Err(ContractError::Validation("worker cannot be empty".into()));
        }
        if self.workspace.trim().is_empty() {
            return Err(ContractError::Validation(
                "workspace cannot be empty".into(),
            ));
        }
        Ok(())
    }

    /// Canonical JSON representation of this contract: sorted object keys,
    /// fixed field order (implied by key sorting), and no incidental
    /// whitespace. Two contracts that are logically identical always
    /// produce byte-identical canonical JSON, regardless of the order in
    /// which fields appeared in the original input.
    ///
    /// This works by round-tripping through [`serde_json::Value`]: this
    /// crate does not enable `serde_json`'s `preserve_order` feature
    /// anywhere in the workspace, so `serde_json::Map` is backed by a
    /// `BTreeMap` and iterates keys in sorted order at every nesting level.
    /// Re-serializing that value with `serde_json::to_string` (compact, no
    /// pretty-printing) yields the deterministic canonical bytes.
    pub fn canonical_json(&self) -> Result<String, ContractError> {
        let value = serde_json::to_value(self)?;
        Ok(serde_json::to_string(&value)?)
    }

    /// Canonical byte representation, as used for hashing.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ContractError> {
        Ok(self.canonical_json()?.into_bytes())
    }

    /// Deterministic policy hash: SHA-256 over the canonical JSON bytes,
    /// hex-encoded. The same logical contract always yields the same hash;
    /// changing any policy field changes the hash.
    pub fn policy_hash(&self) -> Result<String, ContractError> {
        let bytes = self.canonical_bytes()?;
        let digest = Sha256::digest(&bytes);
        Ok(hex::encode(digest))
    }

    /// Human-readable, multi-line summary of the key policy fields.
    pub fn explain(&self) -> String {
        let lines = [
            format!("Execution Contract v{}", self.schema_version),
            format!("task_id: {}", self.task_id),
            format!("task: {}", self.task),
            format!("worker: {}", self.worker),
            format!("workspace: {}", self.workspace),
            format!("network_policy: {}", self.network_policy.describe()),
            format!("provider_policy: {}", self.provider_policy.describe()),
            format!("model_policy: {}", self.model_policy.describe()),
            format!("data_boundary: {}", self.data_boundary.describe()),
            format!("commit_policy: {}", self.commit_policy.describe()),
            format!("push_policy: {}", self.push_policy.describe()),
            format!("approvals_required: {}", self.approvals_required.describe()),
            format!(
                "allowed_tools: {} tool(s), denied_tools: {} tool(s)",
                self.allowed_tools.len(),
                self.denied_tools.len()
            ),
            format!(
                "allowed_paths: {} pattern(s), denied_paths: {} pattern(s)",
                self.allowed_paths.len(),
                self.denied_paths.len()
            ),
            format!(
                "limits: max_tool_rounds={}, max_runtime={}s, max_files_changed={}, max_output_bytes={}",
                self.max_tool_rounds,
                self.max_runtime,
                self.max_files_changed,
                self.max_output_bytes
            ),
            format!(
                "audit_policy: log_tool_calls={}, log_args={}, log_output={}",
                self.audit_policy.log_tool_calls,
                self.audit_policy.log_args,
                self.audit_policy.log_output
            ),
        ];
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_toml() -> &'static str {
        r#"
schema_version = 1
task_id = "task-001"
task = "Implement feature X"
worker = "worker-a"
workspace = "/workspace/example"
"#
    }

    fn full_toml() -> &'static str {
        r#"
schema_version = 1
task_id = "task-001"
task = "Implement feature X"
worker = "worker-a"
workspace = "/workspace/example"
allowed_tools = ["file_read", "file_write"]
denied_tools = ["shell"]
allowed_paths = ["/workspace/example"]
denied_paths = ["/workspace/example/.secrets"]
network_policy = "deny"
provider_policy = "local_only"
data_boundary = "local_only"
max_tool_rounds = 10
max_runtime = 300
max_files_changed = 5
max_output_bytes = 500000
commit_policy = "never"
push_policy = "never"

[model_policy]
allow_list = { models = ["local-model-a"] }

[approvals_required]
list = ["commit", "push"]

[audit_policy]
log_tool_calls = true
log_args = true
log_output = false
"#
    }

    #[test]
    fn parse_valid_contract() {
        let contract = ExecutionContract::parse(full_toml()).expect("should parse");
        assert_eq!(contract.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(contract.task_id, "task-001");
        assert_eq!(contract.allowed_tools, vec!["file_read", "file_write"]);
        assert_eq!(contract.network_policy, NetworkPolicy::Deny);
        assert_eq!(contract.max_tool_rounds, 10);
    }

    #[test]
    fn reject_unknown_field() {
        let input = format!("{}\nthis_field_does_not_exist = true\n", minimal_toml());
        let result = ExecutionContract::parse(&input);
        assert!(result.is_err(), "unknown field must be rejected");
    }

    #[test]
    fn fail_closed_on_missing_security_fields() {
        let contract = ExecutionContract::parse(minimal_toml()).expect("should parse");
        // Absent network_policy must default to deny, not to an implicit allow.
        assert_eq!(contract.network_policy, NetworkPolicy::Deny);
        // Absent provider_policy must default to local_only.
        assert_eq!(contract.provider_policy, ProviderPolicy::LocalOnly);
        // Absent model_policy must default to an empty allow list (nothing allowed).
        assert_eq!(
            contract.model_policy,
            ModelPolicy::AllowList { models: vec![] }
        );
        // Absent data_boundary must default to local_only.
        assert_eq!(contract.data_boundary, DataBoundary::LocalOnly);
        // Absent commit/push policy must default to never.
        assert_eq!(contract.commit_policy, CommitPolicy::Never);
        assert_eq!(contract.push_policy, PushPolicy::Never);
        // Absent approvals_required must default to requiring approval for everything.
        assert_eq!(contract.approvals_required, ApprovalScope::All);
        // Absent allowed_tools/allowed_paths must default to empty (nothing allowed).
        assert!(contract.allowed_tools.is_empty());
        assert!(contract.allowed_paths.is_empty());
    }

    #[test]
    fn canonical_serialization_stable_across_reorderings() {
        let reordered = r#"
worker = "worker-a"
task = "Implement feature X"
schema_version = 1
workspace = "/workspace/example"
task_id = "task-001"
network_policy = "deny"
"#;
        let a = ExecutionContract::parse(minimal_toml()).unwrap();
        let b = ExecutionContract::parse(reordered).unwrap();
        assert_eq!(a.canonical_json().unwrap(), b.canonical_json().unwrap());
    }

    #[test]
    fn canonical_json_has_sorted_top_level_keys() {
        let contract = ExecutionContract::parse(minimal_toml()).unwrap();
        let json = contract.canonical_json().unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let obj = value.as_object().unwrap();
        let keys: Vec<&String> = obj.keys().collect();
        let mut sorted_keys = keys.clone();
        sorted_keys.sort();
        assert_eq!(keys, sorted_keys);
    }

    #[test]
    fn hash_determinism_same_contract_same_hash() {
        let a = ExecutionContract::parse(minimal_toml()).unwrap();
        let b = ExecutionContract::parse(minimal_toml()).unwrap();
        assert_eq!(a.policy_hash().unwrap(), b.policy_hash().unwrap());
    }

    #[test]
    fn hash_determinism_reordered_input_same_hash() {
        let reordered = r#"
task_id = "task-001"
worker = "worker-a"
schema_version = 1
task = "Implement feature X"
workspace = "/workspace/example"
"#;
        let a = ExecutionContract::parse(minimal_toml()).unwrap();
        let b = ExecutionContract::parse(reordered).unwrap();
        assert_eq!(a.policy_hash().unwrap(), b.policy_hash().unwrap());
    }

    #[test]
    fn hash_changes_when_policy_changes() {
        let a = ExecutionContract::parse(minimal_toml()).unwrap();
        let mut b = a.clone();
        b.network_policy = NetworkPolicy::Allow;
        assert_ne!(a.policy_hash().unwrap(), b.policy_hash().unwrap());
    }

    #[test]
    fn explain_contains_key_policy_fields() {
        let contract = ExecutionContract::parse(full_toml()).unwrap();
        let explanation = contract.explain();
        assert!(explanation.contains("task_id: task-001"));
        assert!(explanation.contains("network_policy: deny"));
        assert!(explanation.contains("provider_policy: local_only"));
        assert!(explanation.contains("commit_policy: never"));
        assert!(explanation.contains("push_policy: never"));
    }

    #[test]
    fn validate_rejects_unsupported_schema_version() {
        let mut contract = ExecutionContract::parse(minimal_toml()).unwrap();
        contract.schema_version = 999;
        assert!(contract.validate().is_err());
    }

    #[test]
    fn validate_rejects_empty_required_strings() {
        let mut contract = ExecutionContract::parse(minimal_toml()).unwrap();
        contract.task_id = "  ".to_string();
        assert!(contract.validate().is_err());
    }
}
