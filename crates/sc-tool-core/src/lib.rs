//! Core tool traits, registry, permissions, and approval gates for SC Node.

use async_trait::async_trait;
use sc_config::{PermissionsConfig, ToolPermission, WorkspaceConfig};
use sc_message_types::{SessionId, ToolDefinition, ToolResult};
use std::collections::HashMap;
use std::path::PathBuf;
use thiserror::Error;

/// Context passed to every tool execution.
#[derive(Debug, Clone)]
pub struct ToolContext {
    pub session_id: SessionId,
    pub working_dir: PathBuf,
    pub permissions: ToolPermissions,
}

/// Tool execution permissions derived from config.
#[derive(Debug, Clone)]
pub struct ToolPermissions {
    pub default_policy: String,
    pub tools: HashMap<String, ToolPermission>,
    pub workspace_config: WorkspaceConfig,
}

/// Tools that fall back to the shared "file" permission entry when no
/// entry is registered under their own exact name.
const FILE_FAMILY_TOOLS: &[&str] = &["read_file", "write_file", "list_dir"];

impl ToolPermissions {
    pub fn new(permissions: &PermissionsConfig, workspace_config: WorkspaceConfig) -> Self {
        Self {
            default_policy: permissions.default_policy.clone(),
            tools: permissions.tools.clone(),
            workspace_config,
        }
    }

    pub fn from_config(permissions: &PermissionsConfig, workspace_config: WorkspaceConfig) -> Self {
        Self::new(permissions, workspace_config)
    }

    /// Resolve the `ToolPermission` entry for a tool name.
    ///
    /// Looks up the exact tool name first. If no entry is registered and
    /// the tool is one of the file-family tools (`read_file`, `write_file`,
    /// `list_dir`), falls back to the shared "file" entry. Tools that have
    /// their own exact entry are never affected by the fallback.
    pub fn entry_for(&self, tool_name: &str) -> Option<&ToolPermission> {
        if let Some(entry) = self.tools.get(tool_name) {
            return Some(entry);
        }
        if FILE_FAMILY_TOOLS.contains(&tool_name) {
            return self.tools.get("file");
        }
        None
    }

    /// Effective policy string for a tool: the resolved entry's policy
    /// (see `entry_for`, including file-family fallback), or
    /// `default_policy` if no entry applies.
    pub fn policy_for(&self, tool_name: &str) -> &str {
        self.entry_for(tool_name)
            .map(|p| p.policy.as_str())
            .unwrap_or(self.default_policy.as_str())
    }

    pub fn get_policy(&self, tool_name: &str) -> Option<&ToolPermission> {
        self.entry_for(tool_name)
    }
}

/// Result of a tool execution.
#[derive(Debug, Clone)]
pub struct ToolExecutionResult {
    pub output: String,
    pub is_error: bool,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
}

/// Tool errors.
#[derive(Debug, Error)]
pub enum ToolError {
    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Path not allowed: {0}")]
    PathNotAllowed(String),

    #[error("Execution failed: {0}")]
    ExecutionFailed(String),

    #[error("Tool not found: {0}")]
    ToolNotFound(String),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Sandbox error: {0}")]
    Sandbox(#[from] sc_sandbox::SandboxError),

    #[error("Other tool error: {0}")]
    Other(String),
}

/// Decision returned by the permission checker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    Allow,
    Ask(String),
    Deny(String),
}

/// Decision from an approval gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    Allow,
    Deny,
    AllowAll,
}

/// Approval gate trait. Later this can be implemented by CLI, TUI, GUI, or API.
#[async_trait]
pub trait ApprovalGate: Send + Sync {
    async fn request_approval(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
        policy: &str,
        reason: &str,
    ) -> ApprovalDecision;
}

/// Basic CLI approval gate.
pub struct CliApprovalGate {
    pub auto_approve_all: bool,
}

impl CliApprovalGate {
    pub fn new() -> Self {
        Self {
            auto_approve_all: false,
        }
    }
}

impl Default for CliApprovalGate {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ApprovalGate for CliApprovalGate {
    async fn request_approval(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
        policy: &str,
        reason: &str,
    ) -> ApprovalDecision {
        if self.auto_approve_all {
            return ApprovalDecision::AllowAll;
        }

        println!();
        println!("[Approval Required]");
        println!("Tool: {tool_name}");
        println!("Policy: {policy}");
        println!("Reason: {reason}");
        println!(
            "Args: {}",
            serde_json::to_string_pretty(args).unwrap_or_default()
        );
        print!("Allow? [y/N/a] (a = allow all for this session): ");

        use std::io::{self, Write};
        let _ = io::stdout().flush();

        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_ok() {
            match input.trim().to_lowercase().as_str() {
                "y" | "yes" => ApprovalDecision::Allow,
                "a" | "all" => ApprovalDecision::AllowAll,
                _ => ApprovalDecision::Deny,
            }
        } else {
            ApprovalDecision::Deny
        }
    }
}

/// Core tool trait.
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;

    fn description(&self) -> &str;

    fn parameters_schema(&self) -> serde_json::Value;

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.parameters_schema(),
        }
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        context: ToolContext,
    ) -> Result<ToolResult, ToolError>;
}

/// Registry for available tools.
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|tool| tool.as_ref())
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.values().map(|tool| tool.definition()).collect()
    }

    pub fn names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Check whether a tool call should be allowed, denied, or require approval.
///
/// Evaluation order:
/// 1. Resolve the effective policy: the tool's `ToolPermission.policy` if an
///    entry exists (with file-family fallback, see `ToolPermissions::entry_for`),
///    else `default_policy`. The policy string is matched case-insensitively;
///    `{allow,allowed}` -> Allow, `{deny,denied}` -> Deny,
///    `{ask,approval,require_approval,approval_required}` -> Ask, and any
///    other/unrecognized string fails safe to Ask (never to Allow).
/// 2. A resolved policy of Deny short-circuits immediately as
///    `Deny("... denied by policy")`; patterns are never evaluated for a
///    tool that is denied by policy.
/// 3. If no entry is registered for the tool, or the entry has empty
///    `allow_patterns` and `deny_patterns`, the resolved policy is returned
///    as-is (fast path, no pattern evaluation).
/// 4. Otherwise a match target is derived from `args` (see `derive_target`).
///    If it cannot be derived (missing/malformed/empty cmd or path,
///    non-object args, or no extractor registered for this tool), the call
///    is denied (fail-closed).
/// 5. Deny patterns are evaluated first; any match denies the call
///    immediately (deny always wins over allow).
/// 6. If `allow_patterns` is non-empty, at least one must match or the call
///    is denied ("does not match any allow pattern"); if it matches, the
///    resolved policy is returned. If `allow_patterns` is empty (and no
///    deny pattern matched), the resolved policy is returned.
pub fn check_permission(
    tool_name: &str,
    args: &serde_json::Value,
    permissions: &ToolPermissions,
) -> PermissionDecision {
    let policy = Policy::from_raw(permissions.policy_for(tool_name));

    // Deny by policy always wins, before any pattern is evaluated.
    if policy == Policy::Deny {
        return PermissionDecision::Deny(format!("Tool '{tool_name}' is denied by policy"));
    }

    let policy_decision = if policy == Policy::Allow {
        PermissionDecision::Allow
    } else {
        PermissionDecision::Ask(format!("Tool '{tool_name}' requires approval"))
    };

    let entry = permissions.get_policy(tool_name);
    let has_patterns = entry
        .map(|p| !p.allow_patterns.is_empty() || !p.deny_patterns.is_empty())
        .unwrap_or(false);

    if !has_patterns {
        return policy_decision;
    }
    let entry = entry.expect("has_patterns is only true when entry is Some");

    let target = match derive_target(tool_name, args) {
        Ok(target) => target,
        Err(reason) => return PermissionDecision::Deny(reason),
    };

    // Deny patterns always win over allow patterns.
    if let Some(matched) = target.first_deny_match(&entry.deny_patterns) {
        return PermissionDecision::Deny(format!(
            "{} matches deny pattern '{}'",
            target.describe(),
            matched
        ));
    }

    if entry.allow_patterns.is_empty() {
        return policy_decision;
    }

    if target.any_allow_match(&entry.allow_patterns) {
        policy_decision
    } else {
        PermissionDecision::Deny(format!(
            "{} does not match any allow pattern",
            target.describe()
        ))
    }
}

/// Fail-safe mapping of raw policy strings. Unrecognized strings map to
/// `Ask` rather than `Allow`, since silently granting full access for a
/// typo'd or future policy keyword would be unsafe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Policy {
    Allow,
    Deny,
    Ask,
}

impl Policy {
    fn from_raw(raw: &str) -> Self {
        match raw.to_ascii_lowercase().as_str() {
            "allow" | "allowed" => Policy::Allow,
            "deny" | "denied" => Policy::Deny,
            "ask" | "approval" | "require_approval" | "approval_required" => Policy::Ask,
            _ => Policy::Ask,
        }
    }
}

/// A normalized match target derived from tool call arguments.
enum MatchTarget {
    /// Normalized shell command (whitespace-collapsed, ASCII-lowercased).
    Shell(String),
    /// Normalized full path plus its final path component (filename).
    File { path: String, filename: String },
}

impl MatchTarget {
    fn describe(&self) -> String {
        match self {
            MatchTarget::Shell(cmd) => format!("command '{cmd}'"),
            MatchTarget::File { path, .. } => format!("path '{path}'"),
        }
    }

    fn first_deny_match<'a>(&self, patterns: &'a [String]) -> Option<&'a str> {
        patterns
            .iter()
            .find(|pattern| self.matches(pattern, true))
            .map(|s| s.as_str())
    }

    fn any_allow_match(&self, patterns: &[String]) -> bool {
        patterns.iter().any(|pattern| self.matches(pattern, false))
    }

    /// Test a single pattern against this target.
    ///
    /// Shell: deny patterns match anywhere in the command (contains);
    /// allow patterns are anchored at the start of the command (prefix).
    /// File: both allow and deny patterns are anchored, whole-string
    /// matches against either the filename or the full normalized path.
    fn matches(&self, pattern: &str, is_deny: bool) -> bool {
        match self {
            MatchTarget::Shell(cmd) => {
                let pattern = pattern.to_ascii_lowercase();
                let wrapped = if is_deny {
                    format!("*{pattern}*")
                } else {
                    format!("{pattern}*")
                };
                glob_match(&wrapped, cmd)
            }
            MatchTarget::File { path, filename } => {
                let pattern = pattern.replace('\\', "/").to_ascii_lowercase();
                glob_match(&pattern, filename) || glob_match(&pattern, path)
            }
        }
    }
}

/// Derive a normalized match target from `args` for tools that have a
/// registered target extractor. Returns `Err(reason)` when the target
/// cannot be derived: no extractor is registered for `tool_name`, `args`
/// is not an object, the relevant field is missing/malformed, or it
/// normalizes to empty.
fn derive_target(tool_name: &str, args: &serde_json::Value) -> Result<MatchTarget, String> {
    match tool_name {
        "shell" => derive_shell_target(args).ok_or_else(|| {
            format!("Tool '{tool_name}' has a missing, empty, or malformed 'cmd' argument")
        }),
        "file" | "read_file" | "write_file" | "list_dir" => {
            derive_file_target(args).ok_or_else(|| {
                format!("Tool '{tool_name}' has a missing, empty, or malformed 'path' argument")
            })
        }
        _ => Err("no target extractor for tool with configured patterns".into()),
    }
}

fn derive_shell_target(args: &serde_json::Value) -> Option<MatchTarget> {
    let cmd_value = args.as_object()?.get("cmd")?;
    let raw = match cmd_value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(items) => {
            let mut parts = Vec::with_capacity(items.len());
            for item in items {
                parts.push(item.as_str()?.to_string());
            }
            parts.join(" ")
        }
        _ => return None,
    };
    let normalized = normalize_shell_command(&raw);
    if normalized.is_empty() {
        return None;
    }
    Some(MatchTarget::Shell(normalized))
}

fn derive_file_target(args: &serde_json::Value) -> Option<MatchTarget> {
    let path_value = args.as_object()?.get("path")?;
    let raw = path_value.as_str()?;
    if raw.is_empty() {
        return None;
    }
    let path = normalize_file_path(raw);
    if path.is_empty() {
        return None;
    }
    let filename = path.rsplit('/').next().unwrap_or(&path).to_string();
    Some(MatchTarget::File { path, filename })
}

/// Collapse every run of ASCII whitespace to a single space, trim leading
/// and trailing whitespace, and ASCII-lowercase.
fn normalize_shell_command(raw: &str) -> String {
    let lowered = raw.to_ascii_lowercase();
    let mut result = String::with_capacity(lowered.len());
    let mut pending_space = false;
    for ch in lowered.chars() {
        if ch.is_ascii_whitespace() {
            if !result.is_empty() {
                pending_space = true;
            }
        } else {
            if pending_space {
                result.push(' ');
                pending_space = false;
            }
            result.push(ch);
        }
    }
    result
}

/// Replace backslashes with forward slashes and ASCII-lowercase.
fn normalize_file_path(raw: &str) -> String {
    raw.replace('\\', "/").to_ascii_lowercase()
}

/// Panic-free, byte-wise, anchored wildcard matcher.
///
/// `*` in `pattern` matches any run of bytes (including none); every other
/// byte is literal. Both `pattern` and `text` are expected to already be
/// ASCII-lowercased by the caller. Uses the classic two-pointer wildcard
/// algorithm with bounded backtracking: no recursion, and no string range
/// slicing, so it cannot panic on UTF-8 boundaries.
fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.as_bytes();
    let text = text.as_bytes();
    let (mut p, mut t) = (0usize, 0usize);
    let mut star_p: Option<usize> = None;
    let mut star_t = 0usize;

    while t < text.len() {
        if p < pattern.len() && pattern[p] == text[t] {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star_p = Some(p);
            star_t = t;
            p += 1;
        } else if let Some(sp) = star_p {
            p = sp + 1;
            star_t += 1;
            t = star_t;
        } else {
            return false;
        }
    }

    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_config::{PermissionsConfig, ToolPermission, WorkspaceConfig};
    use std::collections::HashMap;

    fn create_test_permissions(
        tools: HashMap<String, ToolPermission>,
        default_policy: &str,
    ) -> ToolPermissions {
        let config = PermissionsConfig {
            default_policy: default_policy.into(),
            tools,
        };
        ToolPermissions::from_config(&config, WorkspaceConfig::default())
    }

    #[test]
    fn test_glob_match_anchoring_and_wildcards() {
        // Anchored: no implicit trailing wildcard.
        assert!(glob_match("abc", "abc"));
        assert!(!glob_match("abc", "abcd"));
        assert!(!glob_match("abc", "xabc"));
        // Star matches any run, including empty.
        assert!(glob_match("*", ""));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("abc*", "abcdef"));
        assert!(glob_match("*abc", "xxabc"));
        // Empty pattern only matches empty text.
        assert!(glob_match("", ""));
        assert!(!glob_match("", "x"));
        // Multi-star backtracking correctness (not exponential).
        assert!(glob_match("*a*a*b", "zaxayb"));
        assert!(!glob_match("*a*a*b", "zaxyb"));
    }

    #[test]
    fn test_check_permission_allow() {
        let mut tools = HashMap::new();
        tools.insert(
            "read_file".into(),
            ToolPermission {
                policy: "allow".into(),
                allow_patterns: vec![],
                deny_patterns: vec![],
            },
        );
        let permissions = create_test_permissions(tools, "deny");

        let decision = check_permission("read_file", &serde_json::json!({}), &permissions);
        assert!(matches!(decision, PermissionDecision::Allow));
    }

    #[test]
    fn test_check_permission_deny() {
        let mut tools = HashMap::new();
        tools.insert(
            "write_file".into(),
            ToolPermission {
                policy: "deny".into(),
                allow_patterns: vec![],
                deny_patterns: vec![],
            },
        );
        let permissions = create_test_permissions(tools, "allow");

        let decision = check_permission("write_file", &serde_json::json!({}), &permissions);
        assert!(matches!(decision, PermissionDecision::Deny(_)));
    }

    #[test]
    fn test_check_permission_ask() {
        let mut tools = HashMap::new();
        tools.insert(
            "shell".into(),
            ToolPermission {
                policy: "ask".into(),
                allow_patterns: vec![],
                deny_patterns: vec![],
            },
        );
        let permissions = create_test_permissions(tools, "deny");

        let decision = check_permission("shell", &serde_json::json!({}), &permissions);
        assert!(matches!(decision, PermissionDecision::Ask(_)));
    }

    #[test]
    fn test_check_permission_unknown_tool_uses_default() {
        let tools = HashMap::new();
        let permissions = create_test_permissions(tools, "allow");

        let decision = check_permission("unknown_tool", &serde_json::json!({}), &permissions);
        assert!(matches!(decision, PermissionDecision::Allow));
    }

    #[test]
    fn test_check_permission_unknown_tool_default_deny() {
        let tools = HashMap::new();
        let permissions = create_test_permissions(tools, "deny");

        let decision = check_permission("unknown_tool", &serde_json::json!({}), &permissions);
        assert!(matches!(decision, PermissionDecision::Deny(_)));
    }

    #[test]
    fn test_check_permission_deny_policy_wins_over_matching_allow_pattern() {
        let mut tools = HashMap::new();
        tools.insert(
            "shell".into(),
            ToolPermission {
                policy: "deny".into(),
                allow_patterns: vec!["cargo ".into()],
                deny_patterns: vec![],
            },
        );
        let permissions = create_test_permissions(tools, "allow");

        let args = serde_json::json!({"cmd": "cargo test"});
        let decision = check_permission("shell", &args, &permissions);
        // Policy Deny short-circuits before patterns are ever evaluated.
        assert!(matches!(decision, PermissionDecision::Deny(_)));
    }

    #[test]
    fn test_check_permission_web_has_no_extractor_denied_even_with_allow_policy() {
        let mut tools = HashMap::new();
        tools.insert(
            "web".into(),
            ToolPermission {
                policy: "allow".into(),
                allow_patterns: vec![],
                deny_patterns: vec!["localhost".into()],
            },
        );
        let permissions = create_test_permissions(tools, "allow");

        let decision = check_permission("web", &serde_json::json!({}), &permissions);
        match decision {
            PermissionDecision::Deny(reason) => {
                assert!(reason.contains("no target extractor"));
            }
            other => panic!("expected Deny, got {other:?}"),
        }
    }

    #[test]
    fn test_check_permission_unknown_policy_string_fails_safe_to_ask() {
        let mut tools = HashMap::new();
        tools.insert(
            "shell".into(),
            ToolPermission {
                policy: "weird".into(),
                allow_patterns: vec![],
                deny_patterns: vec![],
            },
        );
        let permissions = create_test_permissions(tools, "allow");

        let decision = check_permission("shell", &serde_json::json!({}), &permissions);
        assert!(matches!(decision, PermissionDecision::Ask(_)));
    }

    #[test]
    fn test_check_permission_read_file_family_fallback_to_file_entry_deny() {
        let mut tools = HashMap::new();
        tools.insert(
            "file".into(),
            ToolPermission {
                policy: "ask".into(),
                allow_patterns: vec!["*.md".into()],
                deny_patterns: vec!["secret.key".into()],
            },
        );
        let permissions = create_test_permissions(tools, "deny");

        let args = serde_json::json!({"path": "secret.key"});
        let decision = check_permission("read_file", &args, &permissions);
        assert!(matches!(decision, PermissionDecision::Deny(_)));
    }

    #[test]
    fn test_check_permission_read_file_family_fallback_to_file_entry_allow_maps_to_policy() {
        let mut tools = HashMap::new();
        tools.insert(
            "file".into(),
            ToolPermission {
                policy: "ask".into(),
                allow_patterns: vec!["*.md".into()],
                deny_patterns: vec!["secret.key".into()],
            },
        );
        let permissions = create_test_permissions(tools, "deny");

        let args = serde_json::json!({"path": "README.md"});
        let decision = check_permission("read_file", &args, &permissions);
        assert!(matches!(decision, PermissionDecision::Ask(_)));
    }
}
