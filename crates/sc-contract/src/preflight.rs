//! Execution preflight: evaluate a proposed plan against a contract
//! *without executing anything*.
//!
//! Every function in this module is a pure, side-effect-free evaluation
//! over in-memory data: no filesystem access, no process spawning, no
//! network calls. Wiring this into an actual execution loop (deciding what
//! to do with a [`PreflightReport`]) is explicitly out of scope here -
//! callers decide how to act on the report.

use serde::{Deserialize, Serialize};

use crate::{
    ApprovalScope, CommitPolicy, ExecutionContract, ModelPolicy, NetworkPolicy, ProviderPolicy,
    PushPolicy,
};

/// Identifiers treated as "local" providers for the purposes of
/// [`ProviderPolicy::LocalOnly`] and cloud-data-transfer detection.
const LOCAL_PROVIDER_NAMES: &[&str] = &["ollama", "local"];

/// Hosts treated as staying on the local machine (not cloud data transfer).
const LOCAL_HOSTS: &[&str] = &["localhost", "127.0.0.1", "::1"];

/// A proposed plan of action, described *before* any execution happens.
///
/// This is intentionally plain data (no behavior): whoever builds an
/// execution plan is responsible for populating it accurately; preflight
/// only evaluates what it is given.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProposedPlan {
    /// Paths the plan intends to create/modify/delete.
    pub files_to_change: Vec<String>,
    /// Tool names the plan intends to invoke.
    pub tools_needed: Vec<String>,
    /// Network hosts the plan intends to contact.
    pub network_hosts_needed: Vec<String>,
    /// Provider key the plan intends to use (e.g. `"ollama"`, `"openrouter"`).
    pub provider: String,
    /// Model identifier the plan intends to use.
    pub model: String,
    /// New third-party dependencies the plan would introduce.
    pub new_dependencies: Vec<String>,
    /// Number of commits the plan expects to create.
    pub estimated_commits: u32,
    /// Number of pushes the plan expects to perform.
    pub estimated_pushes: u32,
}

/// Overall risk assessment for a [`ProposedPlan`] against a contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    /// Plan is within contract, no cloud data transfer.
    Low,
    /// Plan is within contract but involves cloud data transfer.
    Medium,
    /// Plan has exactly one contract violation.
    High,
    /// Plan has multiple contract violations.
    Critical,
}

/// A single way in which a [`ProposedPlan`] violates an [`ExecutionContract`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Violation {
    pub category: String,
    pub detail: String,
}

impl Violation {
    fn new(category: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            category: category.into(),
            detail: detail.into(),
        }
    }
}

/// Result of evaluating a [`ProposedPlan`] against an [`ExecutionContract`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreflightReport {
    /// `true` when the plan has no contract violations.
    pub passed: bool,
    pub risk_level: RiskLevel,
    pub violations: Vec<Violation>,
    /// `true` when the plan would move data outside the local machine
    /// (a non-local provider, or a non-local network host).
    pub cloud_data_transfer_detected: bool,
    /// New dependencies the plan would introduce (informational).
    pub new_dependencies_detected: Vec<String>,
    /// Action categories that require human approval before this plan may
    /// proceed (derived from the contract's `approvals_required` plus any
    /// commit/push approval gates that apply).
    pub approvals_required: Vec<String>,
}

/// Evaluate a proposed plan against a contract. Pure function: no side
/// effects, nothing is executed.
pub fn preflight(plan: &ProposedPlan, contract: &ExecutionContract) -> PreflightReport {
    let mut violations = Vec::new();
    let mut approvals_required = Vec::new();

    check_tools(plan, contract, &mut violations);
    check_paths(plan, contract, &mut violations);
    let cloud_data_transfer_detected = check_network(plan, contract, &mut violations);
    check_provider(plan, contract, &mut violations);
    check_model(plan, contract, &mut violations);
    check_data_boundary(cloud_data_transfer_detected, contract, &mut violations);
    check_commits_and_pushes(plan, contract, &mut violations, &mut approvals_required);
    collect_approval_scope(contract, &mut approvals_required);

    if !plan.new_dependencies.is_empty()
        && !approvals_required.contains(&"new_dependencies".to_string())
    {
        approvals_required.push("new_dependencies".to_string());
    }

    approvals_required.sort();
    approvals_required.dedup();

    let risk_level = compute_risk_level(&violations, cloud_data_transfer_detected);

    PreflightReport {
        passed: violations.is_empty(),
        risk_level,
        violations,
        cloud_data_transfer_detected,
        new_dependencies_detected: plan.new_dependencies.clone(),
        approvals_required,
    }
}

fn check_tools(plan: &ProposedPlan, contract: &ExecutionContract, violations: &mut Vec<Violation>) {
    for tool in &plan.tools_needed {
        if contract.denied_tools.iter().any(|denied| denied == tool) {
            violations.push(Violation::new(
                "tool_denied",
                format!("tool '{tool}' is explicitly denied by the contract"),
            ));
            continue;
        }
        if !contract.allowed_tools.iter().any(|allowed| allowed == tool) {
            violations.push(Violation::new(
                "tool_not_allowed",
                format!("tool '{tool}' is not in the contract's allowed_tools list"),
            ));
        }
    }
}

/// Derive the literal "root" of a glob-ish path pattern by stripping a
/// trailing `/**`, then a trailing bare `*`, then a trailing `/`.
fn pattern_root(pattern: &str) -> &str {
    pattern
        .trim_end_matches("/**")
        .trim_end_matches('*')
        .trim_end_matches('/')
}

/// Whether `path` is at or under the directory named by `root`, requiring
/// a real path-segment boundary rather than a raw string prefix: `root`
/// must equal the whole path, or be followed immediately by `/`. This is
/// what stops an allowed root like `/workspace/example` from also
/// authorizing an unrelated sibling like `/workspace/example-evil`.
fn is_under_root(root: &str, path: &str) -> bool {
    if root.is_empty() {
        return false;
    }
    path == root || path.starts_with(&format!("{root}/"))
}

/// Whether `path` matches `root` at *some* path-segment boundary, not just
/// at the start of the path. When `filename_prefix` is `true`, `root` only
/// needs to prefix the final component at that boundary (the semantics of
/// a bare trailing `*`, e.g. `.env*` matching `.env.local`); otherwise
/// [`is_under_root`]'s segment-boundary rule applies from that point
/// onward (the semantics of an exact name or a `/**` suffix, e.g.
/// `.git/**` matching anything under a `.git` directory).
fn matches_at_any_depth(root: &str, path: &str, filename_prefix: bool) -> bool {
    if root.is_empty() {
        return false;
    }
    // Path-segment boundaries: the start of the path, and every position
    // immediately following a '/'.
    let boundaries = std::iter::once(0).chain(
        path.char_indices()
            .filter(|&(_, c)| c == '/')
            .map(|(i, _)| i + 1),
    );
    for start in boundaries {
        let tail = &path[start..];
        let hit = if filename_prefix {
            let component = tail.split('/').next().unwrap_or(tail);
            component.starts_with(root)
        } else {
            is_under_root(root, tail)
        };
        if hit {
            return true;
        }
    }
    false
}

/// Whether `path` matches a contract path pattern.
///
/// Two pattern shapes are supported:
///   - A plain pattern (optionally ending in `/**` or `*`) is matched as a
///     root: `path` must equal it, or sit under it at a real path-segment
///     boundary (see [`is_under_root`]). `/workspace/example` matches
///     `/workspace/example/src/lib.rs` but *not*
///     `/workspace/example-evil/file.rs`.
///   - A pattern starting with `**/` matches at *any* depth in `path`, not
///     just from the start (see [`matches_at_any_depth`]), so
///     `**/.git/**`, `**/node_modules/**`, `**/target/**`, and `**/.env*`
///     match regardless of how deep the matching component is nested.
fn path_matches(pattern: &str, path: &str) -> bool {
    if let Some(rest) = pattern.strip_prefix("**/") {
        let filename_prefix = !rest.ends_with("/**") && rest.ends_with('*');
        return matches_at_any_depth(pattern_root(rest), path, filename_prefix);
    }
    is_under_root(pattern_root(pattern), path)
}

fn check_paths(plan: &ProposedPlan, contract: &ExecutionContract, violations: &mut Vec<Violation>) {
    for path in &plan.files_to_change {
        if contract
            .denied_paths
            .iter()
            .any(|deny| path_matches(deny, path))
        {
            violations.push(Violation::new(
                "path_denied",
                format!("path '{path}' matches a denied_paths pattern"),
            ));
            continue;
        }
        if !contract
            .allowed_paths
            .iter()
            .any(|allow| path_matches(allow, path))
        {
            violations.push(Violation::new(
                "path_not_allowed",
                format!("path '{path}' is not covered by the contract's allowed_paths"),
            ));
        }
    }
}

/// Returns `true` when the plan implies data leaving the local machine.
fn is_cloud_data_transfer(plan: &ProposedPlan) -> bool {
    let provider_is_local = LOCAL_PROVIDER_NAMES.contains(&plan.provider.as_str());
    let all_hosts_local = plan
        .network_hosts_needed
        .iter()
        .all(|host| LOCAL_HOSTS.contains(&host.as_str()));
    !provider_is_local || (!plan.network_hosts_needed.is_empty() && !all_hosts_local)
}

fn check_network(
    plan: &ProposedPlan,
    contract: &ExecutionContract,
    violations: &mut Vec<Violation>,
) -> bool {
    match &contract.network_policy {
        NetworkPolicy::Deny => {
            if !plan.network_hosts_needed.is_empty() {
                violations.push(Violation::new(
                    "network_denied",
                    format!(
                        "plan requires network access to {:?} but network_policy is deny",
                        plan.network_hosts_needed
                    ),
                ));
            }
        }
        NetworkPolicy::AllowList { hosts } => {
            for needed in &plan.network_hosts_needed {
                if !hosts.iter().any(|allowed| allowed == needed) {
                    violations.push(Violation::new(
                        "network_not_allowed",
                        format!("host '{needed}' is not in network_policy's allow list"),
                    ));
                }
            }
        }
        NetworkPolicy::Allow => {}
    }

    is_cloud_data_transfer(plan)
}

fn check_provider(
    plan: &ProposedPlan,
    contract: &ExecutionContract,
    violations: &mut Vec<Violation>,
) {
    if plan.provider.is_empty() {
        return;
    }
    match &contract.provider_policy {
        ProviderPolicy::LocalOnly => {
            if !LOCAL_PROVIDER_NAMES.contains(&plan.provider.as_str()) {
                violations.push(Violation::new(
                    "provider_not_allowed",
                    format!(
                        "provider '{}' is not local, but provider_policy is local_only",
                        plan.provider
                    ),
                ));
            }
        }
        ProviderPolicy::AllowList { providers } => {
            if !providers.iter().any(|p| p == &plan.provider) {
                violations.push(Violation::new(
                    "provider_not_allowed",
                    format!(
                        "provider '{}' is not in provider_policy's allow list",
                        plan.provider
                    ),
                ));
            }
        }
        ProviderPolicy::Any => {}
    }
}

fn check_model(plan: &ProposedPlan, contract: &ExecutionContract, violations: &mut Vec<Violation>) {
    if plan.model.is_empty() {
        return;
    }
    match &contract.model_policy {
        ModelPolicy::AllowList { models } => {
            if !models.iter().any(|m| m == &plan.model) {
                violations.push(Violation::new(
                    "model_not_allowed",
                    format!("model '{}' is not in model_policy's allow list", plan.model),
                ));
            }
        }
        ModelPolicy::Any => {}
    }
}

fn check_data_boundary(
    cloud_data_transfer_detected: bool,
    contract: &ExecutionContract,
    violations: &mut Vec<Violation>,
) {
    if cloud_data_transfer_detected && contract.data_boundary == crate::DataBoundary::LocalOnly {
        violations.push(Violation::new(
            "data_boundary_violation",
            "plan would transfer data off the local machine, but data_boundary is local_only",
        ));
    }
}

fn check_commits_and_pushes(
    plan: &ProposedPlan,
    contract: &ExecutionContract,
    violations: &mut Vec<Violation>,
    approvals_required: &mut Vec<String>,
) {
    if plan.estimated_commits > 0 {
        match contract.commit_policy {
            CommitPolicy::Never => violations.push(Violation::new(
                "commit_denied",
                "plan expects to create commits, but commit_policy is never",
            )),
            CommitPolicy::ApprovalRequired => approvals_required.push("commit".to_string()),
            CommitPolicy::Auto => {}
        }
    }
    if plan.estimated_pushes > 0 {
        match contract.push_policy {
            PushPolicy::Never => violations.push(Violation::new(
                "push_denied",
                "plan expects to push, but push_policy is never",
            )),
            PushPolicy::ApprovalRequired => approvals_required.push("push".to_string()),
            PushPolicy::Auto => {}
        }
    }
}

fn collect_approval_scope(contract: &ExecutionContract, approvals_required: &mut Vec<String>) {
    match &contract.approvals_required {
        ApprovalScope::All => approvals_required.push("all_actions".to_string()),
        ApprovalScope::List(items) => approvals_required.extend(items.iter().cloned()),
        ApprovalScope::None => {}
    }
}

fn compute_risk_level(violations: &[Violation], cloud_data_transfer_detected: bool) -> RiskLevel {
    match violations.len() {
        0 if cloud_data_transfer_detected => RiskLevel::Medium,
        0 => RiskLevel::Low,
        1 => RiskLevel::High,
        _ => RiskLevel::Critical,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ApprovalScope, AuditPolicy, CommitPolicy, DataBoundary, ExecutionContract, ModelPolicy,
        NetworkPolicy, ProviderPolicy, PushPolicy,
    };

    fn base_contract() -> ExecutionContract {
        ExecutionContract {
            schema_version: crate::CURRENT_SCHEMA_VERSION,
            task_id: "task-001".to_string(),
            task: "Implement feature X".to_string(),
            worker: "worker-a".to_string(),
            workspace: "/workspace/example".to_string(),
            allowed_tools: vec!["file_read".to_string(), "file_write".to_string()],
            denied_tools: vec!["shell".to_string()],
            allowed_paths: vec!["/workspace/example".to_string()],
            denied_paths: vec!["/workspace/example/.secrets".to_string()],
            network_policy: NetworkPolicy::Deny,
            provider_policy: ProviderPolicy::LocalOnly,
            model_policy: ModelPolicy::AllowList {
                models: vec!["local-model-a".to_string()],
            },
            data_boundary: DataBoundary::LocalOnly,
            max_tool_rounds: 10,
            max_runtime: 300,
            max_files_changed: 5,
            max_output_bytes: 500_000,
            approvals_required: ApprovalScope::List(vec!["commit".to_string()]),
            commit_policy: CommitPolicy::ApprovalRequired,
            push_policy: PushPolicy::Never,
            audit_policy: AuditPolicy::default(),
        }
    }

    fn clean_plan() -> ProposedPlan {
        ProposedPlan {
            files_to_change: vec!["/workspace/example/src/lib.rs".to_string()],
            tools_needed: vec!["file_write".to_string()],
            network_hosts_needed: vec![],
            provider: "ollama".to_string(),
            model: "local-model-a".to_string(),
            new_dependencies: vec![],
            estimated_commits: 0,
            estimated_pushes: 0,
        }
    }

    #[test]
    fn clean_plan_passes() {
        let report = preflight(&clean_plan(), &base_contract());
        assert!(report.passed);
        assert!(report.violations.is_empty());
        assert_eq!(report.risk_level, RiskLevel::Low);
        assert!(!report.cloud_data_transfer_detected);
    }

    #[test]
    fn plan_violating_contract_is_flagged() {
        let mut plan = clean_plan();
        plan.tools_needed.push("shell".to_string());
        plan.files_to_change.push("/etc/passwd".to_string());

        let report = preflight(&plan, &base_contract());
        assert!(!report.passed);
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.category == "tool_denied")
        );
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.category == "path_not_allowed")
        );
        assert_eq!(report.risk_level, RiskLevel::Critical);
    }

    #[test]
    fn cloud_data_transfer_is_detected() {
        let mut plan = clean_plan();
        plan.provider = "openrouter".to_string();
        plan.network_hosts_needed.push("openrouter.ai".to_string());

        let report = preflight(&plan, &base_contract());
        assert!(report.cloud_data_transfer_detected);
        // network_policy = deny and provider_policy = local_only both fire.
        assert!(!report.passed);
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.category == "data_boundary_violation")
        );
    }

    #[test]
    fn risk_level_computed_for_single_violation() {
        let mut plan = clean_plan();
        plan.tools_needed.push("shell".to_string());

        let report = preflight(&plan, &base_contract());
        assert_eq!(report.violations.len(), 1);
        assert_eq!(report.risk_level, RiskLevel::High);
    }

    #[test]
    fn approvals_required_reflect_contract_and_plan() {
        let mut plan = clean_plan();
        plan.estimated_commits = 1;
        plan.new_dependencies.push("some-crate".to_string());

        let report = preflight(&plan, &base_contract());
        assert!(report.approvals_required.contains(&"commit".to_string()));
        assert!(
            report
                .approvals_required
                .contains(&"new_dependencies".to_string())
        );
    }

    #[test]
    fn push_denied_when_policy_is_never() {
        let mut plan = clean_plan();
        plan.estimated_pushes = 1;

        let report = preflight(&plan, &base_contract());
        assert!(!report.passed);
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.category == "push_denied")
        );
    }

    #[test]
    fn path_matches_root_requires_segment_boundary() {
        // A legitimate subdirectory/file under the root matches.
        assert!(path_matches("/workspace/example", "/workspace/example"));
        assert!(path_matches(
            "/workspace/example",
            "/workspace/example/src/lib.rs"
        ));
        // An unrelated sibling that merely shares the prefix as raw text
        // must NOT match: /workspace/example must not authorize
        // /workspace/example-evil.
        assert!(!path_matches(
            "/workspace/example",
            "/workspace/example-evil/file.rs"
        ));
        assert!(!path_matches(
            "/workspace/example",
            "/workspace/example-evil"
        ));
    }

    #[test]
    fn evil_sibling_path_is_not_authorized_by_allowed_root() {
        let mut plan = clean_plan();
        plan.files_to_change
            .push("/workspace/example-evil/payload.rs".to_string());

        let report = preflight(&plan, &base_contract());
        assert!(!report.passed);
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.category == "path_not_allowed")
        );
    }

    #[test]
    fn path_matches_tail_glob_matches_at_any_depth() {
        // Bare "**/name" (no trailing wildcard) requires an exact final
        // path component, at any depth.
        assert!(path_matches("**/.env", "/workspace/x/.env"));
        assert!(path_matches("**/.env", ".env"));
        assert!(!path_matches("**/.env", "/workspace/x/.envrc"));

        // "**/name*" is a filename-prefix glob: it matches any component
        // at any depth that starts with the given prefix.
        assert!(path_matches("**/.env*", "/workspace/x/.env"));
        assert!(path_matches("**/.env*", "/workspace/x/.env.local"));
        assert!(!path_matches("**/.env*", "/workspace/x/other.env"));

        // "**/name/**" matches anything under a directory named `name` at
        // any depth.
        assert!(path_matches("**/.git/**", "/workspace/example/.git/config"));
        assert!(path_matches(
            "**/node_modules/**",
            "/workspace/example/node_modules/pkg/index.js"
        ));
        assert!(path_matches(
            "**/target/**",
            "/workspace/example/crate/target/debug/build"
        ));
        assert!(!path_matches(
            "**/node_modules/**",
            "/workspace/example/src/lib.rs"
        ));
    }

    #[test]
    fn default_style_denied_patterns_fire_via_preflight() {
        let mut contract = base_contract();
        contract.allowed_paths = vec!["/workspace/example".to_string()];
        contract.denied_paths = vec![
            "**/.git/**".to_string(),
            "**/.env*".to_string(),
            "**/node_modules/**".to_string(),
            "**/target/**".to_string(),
        ];

        let mut plan = clean_plan();
        plan.files_to_change = vec!["/workspace/example/.env".to_string()];

        let report = preflight(&plan, &contract);
        assert!(!report.passed);
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.category == "path_denied")
        );
    }
}
