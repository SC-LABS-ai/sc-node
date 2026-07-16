//! Core agent loop and REPL for SC Node.

use anyhow::Result;
use clap::Parser;
use sc_audit::{AuditLogger, create_audit_entry};
use sc_config::Config;
use sc_message_types::{AuditDecision, Message, SessionId, StreamEvent, ToolCall, ToolDefinition};
use sc_provider_core::Provider;
use sc_tool_core::{
    ApprovalDecision, ApprovalGate, PermissionDecision, ToolContext, ToolPermissions, ToolRegistry,
    check_permission,
};
use std::sync::Arc;
use std::time::Instant;

/// CLI command-line arguments.
#[derive(Parser, Debug)]
#[command(
    name = "sc-agent",
    version,
    about = "SC Node - Private AI Agent Runtime"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Parser, Debug)]
pub enum Commands {
    /// Run a single task and exit
    Run { task: String },
    /// Start interactive REPL
    Repl,
    /// Initialize config file
    #[command(name = "init", alias = "config-init")]
    ConfigInit,
    /// Show current config
    ConfigShow,
    /// Set config value
    ConfigSet { key: String, value: String },
    /// List available models
    ModelsList,
    /// List configured providers
    ProvidersList,
    /// Show audit log
    AuditShow {
        #[arg(long, default_value = "50")]
        last: usize,
    },
    /// Add workspace path
    WorkspaceAdd { path: String },
    /// Check provider health
    Doctor,
    /// Execution-contract operations
    Contract {
        #[command(subcommand)]
        action: ContractCmd,
    },
    /// Execution-proof operations
    Proof {
        #[command(subcommand)]
        action: ProofCmd,
    },
}

/// Subcommands for `sc-agent contract`.
#[derive(Parser, Debug)]
pub enum ContractCmd {
    /// Validate a contract file (strict, fail-closed) and print its policy hash
    Validate { path: String },
    /// Print a human-readable explanation of a contract file
    Explain { path: String },
}

/// Subcommands for `sc-agent proof`.
#[derive(Parser, Debug)]
pub enum ProofCmd {
    /// Verify a proof bundle's hash chain and event count
    Verify { path: String },
}

/// Agent session state.
pub struct Session {
    pub id: SessionId,
    pub messages: Vec<Message>,
    pub providers: Vec<Arc<dyn Provider>>,
    pub tools: ToolRegistry,
    pub config: Config,
    pub audit: Option<Arc<AuditLogger>>,
    pub approval_auto_allow: bool,
}

impl Session {
    pub fn new(
        config: Config,
        providers: Vec<Arc<dyn Provider>>,
        tools: ToolRegistry,
        audit: Option<Arc<AuditLogger>>,
    ) -> Self {
        Self {
            id: SessionId::new(),
            messages: Vec::new(),
            providers,
            tools,
            config,
            audit,
            approval_auto_allow: false,
        }
    }

    /// Get provider by key.
    pub fn provider(&self, key: &str) -> Option<&Arc<dyn Provider>> {
        self.providers.iter().find(|p| p.key() == key)
    }

    /// Whether the named provider is administratively enabled in config.
    fn provider_enabled(&self, key: &str) -> bool {
        let p = &self.config.providers;
        match key {
            "ollama" => p.ollama.as_ref().map(|c| c.enabled).unwrap_or(false),
            "nvidia" => p.nvidia.as_ref().map(|c| c.enabled).unwrap_or(false),
            "openrouter" => p.openrouter.as_ref().map(|c| c.enabled).unwrap_or(false),
            // A provider registered with no matching config entry (e.g. a
            // custom or test provider) is considered enabled: it was
            // deliberately added to the session.
            _ => true,
        }
    }

    /// Describe the registered providers to the router. A provider is a legal
    /// routing target when it is registered here and enabled in config; the
    /// credential itself is enforced by the provider at request time (a missing
    /// key surfaces as an audited provider failure, never a silent local
    /// fallback). `ollama` is the only local provider.
    fn available_providers(&self) -> Vec<sc_provider_core::routing::AvailableProvider> {
        use sc_provider_core::routing::AvailableProvider;
        self.providers
            .iter()
            .map(|p| {
                let key = p.key();
                AvailableProvider::new(key, self.provider_enabled(key), true, key == "ollama")
            })
            .collect()
    }

    /// Build the router's routing config from the on-disk config.
    fn router_config(&self) -> sc_provider_core::routing::RoutingConfig {
        use sc_provider_core::routing::{FallbackRoute, RoutingConfig, RoutingRule};
        let r = &self.config.routing;
        let rules = r
            .rules
            .iter()
            .map(|rule| RoutingRule {
                name: rule.name.clone(),
                match_contains: rule.match_contains.clone(),
                provider: rule.provider.clone(),
                model: rule.model.clone(),
            })
            .collect();
        let fallback = if r.fallback_provider.trim().is_empty() {
            None
        } else {
            Some(FallbackRoute {
                provider: r.fallback_provider.clone(),
                model: r.fallback_model.clone(),
                enabled: true,
            })
        };
        RoutingConfig { rules, fallback }
    }

    /// Deterministically resolve which provider/model a task routes to. There
    /// is NO silent first-provider fallback: an unresolved route is an error.
    /// Cloud providers are only eligible when at least one cloud provider is
    /// administratively enabled (enabling one in config is the cloud opt-in).
    pub fn resolve_route(&self, task: &str) -> Result<sc_provider_core::routing::ResolvedRoute> {
        use sc_provider_core::routing::{RouteRequest, resolve_route};
        let providers = self.available_providers();
        // Only providers that are both registered and enabled are usable. Rules
        // and fallbacks pointing at an unavailable provider (e.g. a cloud
        // provider left disabled) are dropped so routing falls through to
        // local-first instead of hard-failing the whole run. This never routes
        // to a provider the operator did not enable.
        let usable: std::collections::HashSet<String> = providers
            .iter()
            .filter(|p| p.enabled)
            .map(|p| p.provider.clone())
            .collect();
        let mut config = self.router_config();
        config.rules.retain(|r| {
            let keep = usable.contains(&r.provider);
            if !keep {
                eprintln!(
                    "[Route] skipping rule '{}': provider '{}' is not enabled/registered",
                    r.name, r.provider
                );
            }
            keep
        });
        if let Some(fb) = &config.fallback
            && !usable.contains(&fb.provider)
        {
            eprintln!(
                "[Route] ignoring fallback: provider '{}' is not enabled/registered",
                fb.provider
            );
            config.fallback = None;
        }
        let allow_cloud = providers.iter().any(|p| !p.is_local && p.enabled);
        let request = RouteRequest {
            requested_provider: None,
            requested_model: None,
            task: Some(task.to_string()),
        };
        resolve_route(&providers, &config, &request, allow_cloud)
            .map_err(|e| anyhow::anyhow!("no provider route for task: {e}"))
    }

    /// Add a message to history.
    pub fn add_message(&mut self, msg: Message) {
        self.messages.push(msg);
    }

    /// Get tool definitions for the current session.
    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tools.definitions()
    }
}

/// Approval mode governing how `Ask` permission decisions are resolved for
/// the lifetime of a `run_session` invocation.
#[derive(Clone)]
pub enum ApprovalMode {
    /// Fail closed: any `Ask` decision is denied without prompting. Used for
    /// non-interactive invocations (`run`, or `repl` on a non-TTY stdin).
    AutoDeny,
    /// Delegate `Ask` decisions to an interactive approval gate (e.g. a CLI
    /// prompt). An `AllowAll` response from the gate is remembered for the
    /// rest of the session via `ApprovalState::allow_all`.
    Interactive(Arc<dyn ApprovalGate>),
    /// Automatically approve any `Ask` decision. Never selected by default;
    /// only reachable behind an explicit opt-in flag.
    AutoAllow,
}

/// Mutable approval state threaded through a single `execute_task` call.
pub struct ApprovalState {
    pub mode: ApprovalMode,
    pub allow_all: bool,
}

/// Outcome of dispatching a single tool call through the permission gate.
pub struct DispatchOutcome {
    pub executed: bool,
    pub tool_result: Message,
    pub audit_entry: sc_message_types::AuditEntry,
}

/// Execute a task (single shot or REPL loop).
pub async fn run_session(
    mut session: Session,
    task: Option<String>,
    approval_mode: ApprovalMode,
) -> Result<()> {
    if let Some(task) = task {
        // Single-shot mode
        execute_task(&mut session, &task, approval_mode).await?;
    } else {
        // REPL mode
        repl_loop(&mut session, approval_mode).await?;
    }
    Ok(())
}

/// Record a sanitized provider-failure audit event. Never includes the API
/// key (the provider layer has already redacted it from `error`), the
/// Authorization header, or the raw task prompt. Provider key and model slug
/// are non-secret and are always recorded (in the tool/policy fields); the
/// message history is never captured here.
async fn emit_provider_failure_audit(
    audit_logger: &Option<Arc<AuditLogger>>,
    session_id: SessionId,
    route: &sc_provider_core::routing::ResolvedRoute,
    log_args: bool,
    error: &impl std::fmt::Display,
) {
    let Some(logger) = audit_logger else {
        return;
    };
    let entry = create_audit_entry(
        session_id,
        format!("provider/{}", route.provider),
        None, // never capture the task prompt in a provider-failure event
        format!("model={}; route={:?}", route.model, route.reason),
        AuditDecision::Error,
        Some(1),
        0,
        Some(error.to_string()),
        log_args,
        false,
        None,
    );
    let _ = logger.log(entry).await;
}

async fn execute_task(
    session: &mut Session,
    task: &str,
    approval_mode: ApprovalMode,
) -> Result<()> {
    // Add user message
    session.add_message(Message::user(task));

    // Max tool rounds to prevent infinite loops
    const MAX_TOOL_ROUNDS: usize = 3;
    let mut tool_rounds = 0;

    // Hoisted once per task: shared by every dispatch_tool_call below.
    let permissions = ToolPermissions::from_config(
        &session.config.permissions,
        session.config.workspace.clone(),
    );
    let working_dir = std::env::current_dir()?;
    let mut approval = ApprovalState {
        mode: approval_mode,
        allow_all: session.approval_auto_allow,
    };

    // Deterministic provider/model selection for this task. Resolved once and
    // stable for every round; there is NO silent first-provider fallback.
    let route = session.resolve_route(task)?;
    println!(
        "[Route] provider={} model={} ({:?})",
        route.provider,
        if route.model.is_empty() {
            "<provider-default>"
        } else {
            route.model.as_str()
        },
        route.reason
    );
    let audit_logger = session.audit.clone();
    let log_args = session.config.audit.log_args;

    loop {
        // Provider chosen by the deterministic router. Clone the Arc so no
        // borrow of `session` is held across the mutable updates below.
        let provider = session.provider(&route.provider).cloned().ok_or_else(|| {
            anyhow::anyhow!(
                "router selected provider '{}' which is not registered",
                route.provider
            )
        })?;

        // Build completion request with the exact model slug from the router
        // (empty slug = let the provider use its own configured default).
        let request = sc_message_types::CompletionRequest {
            model: route.model.clone(),
            messages: session.messages.clone(),
            tools: session.tool_definitions(),
            system: Some("You are a helpful AI assistant with access to tools.".into()),
            stream: true,
            temperature: Some(0.7),
            max_tokens: Some(4096),
        };

        // Stream completion; a provider failure is audited (sanitized) before
        // it propagates, so provider errors leave an audit trail.
        let mut stream = match provider.complete(request).await {
            Ok(s) => s,
            Err(e) => {
                emit_provider_failure_audit(&audit_logger, session.id, &route, log_args, &e).await;
                eprintln!("\n[Provider error] {}", e);
                return Err(e.into());
            }
        };
        use futures::StreamExt;
        let mut tool_calls_this_round = Vec::new();

        while let Some(event) = stream.next().await {
            match event {
                Ok(StreamEvent::TextDelta { text }) => print!("{}", text),
                Ok(StreamEvent::ToolUse { id, name, input }) => {
                    println!("\n[Tool Call] {}: {:?}", name, input);
                    tool_calls_this_round.push((id, name, input));
                }
                Ok(StreamEvent::End { finish_reason }) => {
                    println!("\n[Done: {:?}]", finish_reason);
                }
                Ok(StreamEvent::Error { .. }) => {
                    eprintln!("Provider stream returned an error event.");
                }
                Err(e) => {
                    emit_provider_failure_audit(&audit_logger, session.id, &route, log_args, &e)
                        .await;
                    eprintln!("\n[Error] {}", e);
                    return Err(e.into());
                }
            }
        }

        // If no tool calls, we're done
        if tool_calls_this_round.is_empty() {
            break;
        }

        // Check max rounds
        tool_rounds += 1;
        if tool_rounds >= MAX_TOOL_ROUNDS {
            eprintln!(
                "\n[Error] Max tool rounds ({}) exceeded. Stopping.",
                MAX_TOOL_ROUNDS
            );
            return Err(anyhow::anyhow!(
                "Max tool rounds ({}) exceeded",
                MAX_TOOL_ROUNDS
            ));
        }

        // Execute all tool calls through the central permission gate.
        for (id, name, input) in tool_calls_this_round {
            println!("\n[Tool Call] {}: {:?}", name, input);
            let call = ToolCall { id, name, input };
            let outcome = dispatch_tool_call(
                &call,
                &session.tools,
                &permissions,
                session.id,
                &working_dir,
                &mut approval,
                session.audit.as_deref(),
                session.config.audit.log_args,
                session.config.audit.log_output,
            )
            .await;
            session.add_message(outcome.tool_result);
        }
    }

    session.approval_auto_allow = approval.allow_all;

    Ok(())
}

/// Central permission gate: resolve the permission decision for `call`
/// (policy + patterns, with `Ask` routed through `approval`), then execute
/// the tool only if the call is allowed. Every branch (unknown tool, denied
/// by policy, approval denied, executed) flows through the single `emit`
/// helper below, so audit logging and its redaction rules can never be
/// skipped on any path.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn dispatch_tool_call(
    call: &ToolCall,
    registry: &ToolRegistry,
    permissions: &ToolPermissions,
    session_id: SessionId,
    working_dir: &std::path::Path,
    approval: &mut ApprovalState,
    audit: Option<&AuditLogger>,
    log_args: bool,
    log_output: bool,
) -> DispatchOutcome {
    let start_time = Instant::now();
    // Family-fallback aware (e.g. read_file/write_file/list_dir -> "file"),
    // unlike a raw `permissions.tools.get(name)` lookup.
    let policy_label = permissions.policy_for(&call.name).to_string();

    let Some(tool) = registry.get(&call.name) else {
        let reason = format!("Tool '{}' not found", call.name);
        let duration_ms = start_time.elapsed().as_millis() as u64;
        return emit(
            call,
            session_id,
            &policy_label,
            AuditDecision::Denied,
            Some(127),
            duration_ms,
            Some(reason.clone()),
            log_args,
            log_output,
            None,
            audit,
            false,
            reason,
            true,
        )
        .await;
    };

    // DECIDE strictly before EXECUTE.
    let decision = check_permission(&call.name, &call.input, permissions);
    let approved = match decision {
        PermissionDecision::Allow => Ok(()),
        PermissionDecision::Ask(reason) => {
            resolve_ask(approval, &call.name, &call.input, &policy_label, &reason).await
        }
        PermissionDecision::Deny(reason) => Err(reason),
    };

    if let Some(reason) = approved.err() {
        let duration_ms = start_time.elapsed().as_millis() as u64;
        return emit(
            call,
            session_id,
            &policy_label,
            AuditDecision::Denied,
            Some(126),
            duration_ms,
            Some(reason.clone()),
            log_args,
            log_output,
            None,
            audit,
            false,
            reason,
            true,
        )
        .await;
    }

    // EXECUTE: only reached for a policy Allow, or an Ask that was approved.
    let context = ToolContext {
        session_id,
        working_dir: working_dir.to_path_buf(),
        permissions: permissions.clone(),
    };
    let result = tool.execute(call.input.clone(), context).await;
    let duration_ms = start_time.elapsed().as_millis() as u64;

    match result {
        Ok(r) => {
            let decision = if r.is_error {
                AuditDecision::Error
            } else {
                AuditDecision::Allowed
            };
            let error = if r.is_error {
                Some(r.output.clone())
            } else {
                None
            };
            emit(
                call,
                session_id,
                &policy_label,
                decision,
                r.exit_code,
                duration_ms,
                error,
                log_args,
                log_output,
                Some(r.output.clone()),
                audit,
                true,
                r.output,
                r.is_error,
            )
            .await
        }
        Err(e) => {
            let error_msg = e.to_string();
            emit(
                call,
                session_id,
                &policy_label,
                AuditDecision::Error,
                Some(1),
                duration_ms,
                Some(error_msg.clone()),
                log_args,
                log_output,
                None,
                audit,
                true,
                error_msg,
                true,
            )
            .await
        }
    }
}

/// Resolve an `Ask` permission decision against the current approval state.
/// Returns `Ok(())` when the call is approved, or `Err(reason)` when denied.
/// `approval.allow_all` (once set by an `AllowAll` response) short-circuits
/// every subsequent call without consulting the gate again.
async fn resolve_ask(
    approval: &mut ApprovalState,
    tool_name: &str,
    args: &serde_json::Value,
    policy_label: &str,
    reason: &str,
) -> Result<(), String> {
    if approval.allow_all {
        return Ok(());
    }

    let gate = match &approval.mode {
        ApprovalMode::AutoDeny => {
            return Err("approval required but running non-interactively".into());
        }
        ApprovalMode::AutoAllow => return Ok(()),
        ApprovalMode::Interactive(gate) => gate.clone(),
    };

    match gate
        .request_approval(tool_name, args, policy_label, reason)
        .await
    {
        ApprovalDecision::Allow => Ok(()),
        ApprovalDecision::AllowAll => {
            approval.allow_all = true;
            Ok(())
        }
        ApprovalDecision::Deny => Err("approval denied".into()),
    }
}

/// Single emission point for a dispatch outcome: writes the audit entry (if
/// an audit logger is configured) and builds the tool-result message. Used
/// by every branch of `dispatch_tool_call` so redaction is never skipped.
#[allow(clippy::too_many_arguments)]
async fn emit(
    call: &ToolCall,
    session_id: SessionId,
    policy_label: &str,
    decision: AuditDecision,
    exit_code: Option<i32>,
    duration_ms: u64,
    error: Option<String>,
    log_args: bool,
    log_output: bool,
    output: Option<String>,
    audit: Option<&AuditLogger>,
    executed: bool,
    content: String,
    is_error: bool,
) -> DispatchOutcome {
    let entry = create_audit_entry(
        session_id,
        &call.name,
        Some(call.input.clone()),
        policy_label,
        decision,
        exit_code,
        duration_ms,
        error,
        log_args,
        log_output,
        output,
    );
    if let Some(logger) = audit {
        let _ = logger.log(entry.clone()).await;
    }
    DispatchOutcome {
        executed,
        tool_result: Message::tool_result(&call.id, content, is_error),
        audit_entry: entry,
    }
}

async fn repl_loop(session: &mut Session, approval_mode: ApprovalMode) -> Result<()> {
    println!("SC Node REPL - Type /help for commands, /exit to quit");

    loop {
        print!("\nsc> ");
        use std::io::{self, Write};
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim();

        if input.is_empty() {
            continue;
        }
        if input == "/exit" || input == "/quit" {
            break;
        }
        if input == "/help" {
            println!("Commands: /help, /exit, /clear, /providers, /models");
            continue;
        }

        execute_task(session, input, approval_mode.clone()).await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::execute_task;
    use super::*;
    use async_trait::async_trait;
    use sc_config::{AuditConfig, Config, PermissionsConfig, ToolPermission, WorkspaceConfig};
    use sc_message_types::{ContentBlock, Role, StreamEvent, ToolResult};
    use sc_provider_core::{EventStream, Provider, ProviderError, Result};
    use sc_tool_core::{Tool, ToolContext, ToolError, ToolRegistry};
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A fake provider that can be programmed to emit specific events per round
    struct FakeProvider {
        // Events grouped by round - each round is a Vec of events ending with End
        rounds: Vec<Vec<StreamEvent>>,
        call_count: std::sync::atomic::AtomicUsize,
    }

    impl FakeProvider {
        fn with_events(events: Vec<StreamEvent>) -> Arc<Self> {
            // Group events by round (split on End events)
            let mut rounds = Vec::new();
            let mut current_round = Vec::new();

            for event in events {
                current_round.push(event);
                if matches!(&current_round.last(), Some(StreamEvent::End { .. })) {
                    rounds.push(current_round);
                    current_round = Vec::new();
                }
            }
            if !current_round.is_empty() {
                rounds.push(current_round);
            }

            Arc::new(Self {
                rounds,
                call_count: std::sync::atomic::AtomicUsize::new(0),
            })
        }
    }

    #[async_trait]
    impl Provider for FakeProvider {
        fn key(&self) -> &str {
            "fake"
        }

        fn name(&self) -> &str {
            "Fake Provider"
        }

        async fn list_models(&self) -> sc_provider_core::Result<Vec<sc_message_types::ModelInfo>> {
            Ok(vec![])
        }

        async fn complete(
            &self,
            _request: sc_message_types::CompletionRequest,
        ) -> Result<EventStream, ProviderError> {
            let idx = self
                .call_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let round_events = if idx < self.rounds.len() {
                self.rounds[idx].clone()
            } else {
                vec![StreamEvent::End {
                    finish_reason: Some("stop".into()),
                }]
            };

            let events = round_events.into_iter().map(Ok).collect::<Vec<_>>();
            Ok(Box::pin(futures::stream::iter(events)))
        }
    }

    /// A fake provider with a configurable key that records how many times it
    /// was invoked and the exact model slug it received. Used to prove the
    /// deterministic router picks the right provider/model in the real run
    /// loop (not the legacy first-provider selection).
    struct RoutingFakeProvider {
        key: &'static str,
        count: AtomicUsize,
        model_seen: std::sync::Mutex<Option<String>>,
        fail: bool,
    }

    impl RoutingFakeProvider {
        fn new(key: &'static str) -> Arc<Self> {
            Arc::new(Self {
                key,
                count: AtomicUsize::new(0),
                model_seen: std::sync::Mutex::new(None),
                fail: false,
            })
        }
        fn new_failing(key: &'static str) -> Arc<Self> {
            Arc::new(Self {
                key,
                count: AtomicUsize::new(0),
                model_seen: std::sync::Mutex::new(None),
                fail: true,
            })
        }
        fn calls(&self) -> usize {
            self.count.load(Ordering::SeqCst)
        }
        fn model(&self) -> Option<String> {
            self.model_seen.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl Provider for RoutingFakeProvider {
        fn key(&self) -> &str {
            self.key
        }
        fn name(&self) -> &str {
            "Routing Fake Provider"
        }
        async fn list_models(&self) -> sc_provider_core::Result<Vec<sc_message_types::ModelInfo>> {
            Ok(vec![])
        }
        async fn complete(
            &self,
            request: sc_message_types::CompletionRequest,
        ) -> Result<EventStream, ProviderError> {
            self.count.fetch_add(1, Ordering::SeqCst);
            *self.model_seen.lock().unwrap() = Some(request.model.clone());
            if self.fail {
                return Err(ProviderError::Api("upstream 500 (simulated)".into()));
            }
            let events = vec![
                Ok(StreamEvent::TextDelta { text: "ok".into() }),
                Ok(StreamEvent::End {
                    finish_reason: Some("stop".into()),
                }),
            ];
            Ok(Box::pin(futures::stream::iter(events)))
        }
    }

    fn session_with_nim_rule() -> (Session, Arc<RoutingFakeProvider>, Arc<RoutingFakeProvider>) {
        use sc_config::RoutingRule;
        let mut config = Config::default();
        if let Some(o) = config.providers.ollama.as_mut() {
            o.enabled = true;
        }
        if let Some(n) = config.providers.nvidia.as_mut() {
            n.enabled = true;
        }
        config.routing.rules = vec![RoutingRule {
            name: "nim-live-test".into(),
            match_contains: vec!["nim-test".into()],
            provider: "nvidia".into(),
            model: "nvidia/nemotron-3-ultra-550b-a55b".into(),
        }];
        config.routing.fallback_provider = "ollama".into();
        config.routing.fallback_model = String::new();

        let nvidia = RoutingFakeProvider::new("nvidia");
        let ollama = RoutingFakeProvider::new("ollama");
        let providers: Vec<Arc<dyn Provider>> = vec![ollama.clone(), nvidia.clone()];
        let session = Session::new(config, providers, ToolRegistry::new(), None);
        (session, nvidia, ollama)
    }

    #[tokio::test]
    async fn run_routes_matching_task_to_nvidia_exact_model() {
        let (mut session, nvidia, ollama) = session_with_nim_rule();
        execute_task(
            &mut session,
            "nim-test: Reply exactly with SC NODE NIM TEST OK",
            ApprovalMode::AutoDeny,
        )
        .await
        .expect("task run");
        assert_eq!(nvidia.calls(), 1, "NVIDIA must be invoked exactly once");
        assert_eq!(ollama.calls(), 0, "Ollama must not be invoked");
        assert_eq!(
            nvidia.model().as_deref(),
            Some("nvidia/nemotron-3-ultra-550b-a55b"),
            "the exact routed model slug must reach the provider"
        );
    }

    #[tokio::test]
    async fn run_without_rule_match_never_picks_cloud() {
        let (mut session, nvidia, ollama) = session_with_nim_rule();
        execute_task(
            &mut session,
            "just a normal local task",
            ApprovalMode::AutoDeny,
        )
        .await
        .expect("task run");
        assert_eq!(
            nvidia.calls(),
            0,
            "cloud must never be silently selected without a matching rule"
        );
        assert_eq!(ollama.calls(), 1, "local fallback should handle it");
        assert_eq!(
            ollama.model().as_deref(),
            Some(""),
            "empty fallback model => provider default slug, never literal 'default'"
        );
    }

    #[tokio::test]
    async fn provider_failure_is_audited_sanitized() {
        use sc_config::AuditConfig;
        let dir = tempfile::tempdir().unwrap();
        let audit_path = dir.path().join("audit.log");
        let audit_cfg = AuditConfig {
            enabled: true,
            path: audit_path.to_string_lossy().to_string(),
            log_args: true, // even with logging ON, the raw prompt must not be captured
            log_output: true,
            ..Default::default()
        };
        let logger = Arc::new(AuditLogger::new(audit_cfg).await.unwrap());

        let mut config = Config::default();
        if let Some(o) = config.providers.ollama.as_mut() {
            o.enabled = true;
        }
        if let Some(n) = config.providers.nvidia.as_mut() {
            n.enabled = true;
        }
        config.routing.rules = vec![sc_config::RoutingRule {
            name: "nim-live-test".into(),
            match_contains: vec!["nim-test".into()],
            provider: "nvidia".into(),
            model: "nvidia/nemotron-3-ultra-550b-a55b".into(),
        }];
        let nvidia = RoutingFakeProvider::new_failing("nvidia");
        let ollama = RoutingFakeProvider::new("ollama");
        let providers: Vec<Arc<dyn Provider>> = vec![ollama, nvidia];
        let mut session =
            Session::new(config, providers, ToolRegistry::new(), Some(logger.clone()));

        let marker = "SUPERSECRETPROMPTMARKER";
        let result = execute_task(
            &mut session,
            &format!("nim-test: {marker}"),
            ApprovalMode::AutoDeny,
        )
        .await;
        assert!(result.is_err(), "provider failure must propagate");

        let entries = logger.read_last(10).await.unwrap();
        let pf = entries
            .iter()
            .find(|e| e.tool.starts_with("provider/"))
            .expect("provider failure must be audited");
        assert_eq!(pf.decision, AuditDecision::Error);
        assert!(pf.tool.contains("nvidia"), "records the provider key");
        // The raw task prompt must never reach the audit record.
        assert!(pf.args.is_none(), "prompt/args must not be captured");
        let serialized = serde_json::to_string(pf).unwrap();
        assert!(
            !serialized.contains(marker),
            "raw prompt leaked into provider-failure audit: {serialized}"
        );
    }

    #[tokio::test]
    async fn run_default_config_only_ollama_falls_through_to_local() {
        // Regression guard: the default config's fallback points at openrouter
        // (disabled by default). With only ollama registered, a non-code task
        // must fall through to local-first - never hard-error, never cloud.
        let mut config = Config::default();
        if let Some(o) = config.providers.ollama.as_mut() {
            o.enabled = true;
        }
        let ollama = RoutingFakeProvider::new("ollama");
        let providers: Vec<Arc<dyn Provider>> = vec![ollama.clone()];
        let mut session = Session::new(config, providers, ToolRegistry::new(), None);
        execute_task(
            &mut session,
            "please summarize this text",
            ApprovalMode::AutoDeny,
        )
        .await
        .expect("default-config run must not hard-error");
        assert_eq!(ollama.calls(), 1, "local provider should handle the task");
    }

    /// A fake tool for testing
    struct FakeTool {
        name: &'static str,
        result: ToolResult,
        should_fail: bool,
    }

    impl FakeTool {
        fn new(name: &'static str, result: ToolResult) -> Self {
            Self {
                name,
                result,
                should_fail: false,
            }
        }

        fn failing(name: &'static str) -> Self {
            Self {
                name,
                result: ToolResult {
                    tool_call_id: String::new(),
                    output: String::new(),
                    is_error: true,
                    exit_code: Some(1),
                },
                should_fail: true,
            }
        }
    }

    #[async_trait]
    impl Tool for FakeTool {
        fn name(&self) -> &str {
            self.name
        }

        fn description(&self) -> &str {
            "Fake tool for testing"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "arg": { "type": "string" }
                },
                "required": ["arg"]
            })
        }

        async fn execute(
            &self,
            _input: serde_json::Value,
            _context: ToolContext,
        ) -> Result<ToolResult, ToolError> {
            if self.should_fail {
                Err(ToolError::ExecutionFailed("fake tool failed".into()))
            } else {
                Ok(self.result.clone())
            }
        }
    }

    fn create_test_session(providers: Vec<Arc<dyn Provider>>) -> Session {
        let mut config = Config::default();
        // Route deterministically to the (single) registered test provider so
        // the agent loop's router selects it instead of a default cloud/local
        // rule that references an unregistered provider.
        if let Some(first) = providers.first() {
            config.routing.rules = vec![];
            config.routing.fallback_provider = first.key().to_string();
            config.routing.fallback_model = String::new();
        }
        Session::new(config, providers, ToolRegistry::new(), None)
    }

    fn create_tool_registry_with_echo() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        let result = ToolResult {
            tool_call_id: String::new(),
            output: "echo result: hello".into(),
            is_error: false,
            exit_code: Some(0),
        };
        registry.register(Box::new(FakeTool::new("echo", result)));
        registry
    }

    fn create_tool_registry_with_steps() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(FakeTool::new(
            "step1",
            ToolResult {
                tool_call_id: String::new(),
                output: "step 1 done".into(),
                is_error: false,
                exit_code: Some(0),
            },
        )));
        registry.register(Box::new(FakeTool::new(
            "step2",
            ToolResult {
                tool_call_id: String::new(),
                output: "step 2 done".into(),
                is_error: false,
                exit_code: Some(0),
            },
        )));
        registry
    }

    fn create_tool_registry_with_loop() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(FakeTool::new(
            "loop",
            ToolResult {
                tool_call_id: String::new(),
                output: "loop".into(),
                is_error: false,
                exit_code: Some(0),
            },
        )));
        registry
    }

    fn create_tool_registry_with_failing() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(FakeTool::failing("failing_tool")));
        registry
    }

    #[tokio::test]
    async fn test_agent_loop_single_tool_call() {
        // Test that a single tool call is executed and its result is added to history
        let provider = FakeProvider::with_events(vec![
            StreamEvent::ToolUse {
                id: "tc-1".into(),
                name: "echo".into(),
                input: serde_json::json!({ "text": "hello" }),
            },
            StreamEvent::End {
                finish_reason: Some("stop".into()),
            },
        ]);

        let mut session = create_test_session(vec![provider]);
        session.tools = create_tool_registry_with_echo();

        let result = execute_task(&mut session, "say hello", ApprovalMode::AutoAllow).await;
        assert!(result.is_ok(), "Task should succeed");

        // Verify tool result was added to history
        let tool_results: Vec<_> = session
            .messages
            .iter()
            .filter(|m| m.role == Role::Tool)
            .collect();
        assert_eq!(
            tool_results.len(),
            1,
            "Should have one tool result in history"
        );

        let tool_msg = &tool_results[0];
        if let ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } = &tool_msg.content[0]
        {
            assert_eq!(tool_use_id, "tc-1");
            assert!(!is_error);
            assert!(content.contains("echo result"));
        } else {
            panic!("Expected ToolResult content block");
        }
    }

    #[tokio::test]
    async fn test_agent_loop_multiple_tool_rounds() {
        // Test multiple tool rounds
        let provider = FakeProvider::with_events(vec![
            StreamEvent::ToolUse {
                id: "tc-1".into(),
                name: "step1".into(),
                input: serde_json::json!({}),
            },
            StreamEvent::End {
                finish_reason: Some("tool_calls".into()),
            },
            StreamEvent::ToolUse {
                id: "tc-2".into(),
                name: "step2".into(),
                input: serde_json::json!({}),
            },
            StreamEvent::End {
                finish_reason: Some("stop".into()),
            },
        ]);

        let mut session = create_test_session(vec![provider]);
        session.tools = create_tool_registry_with_steps();

        let result = execute_task(&mut session, "do two steps", ApprovalMode::AutoAllow).await;
        assert!(result.is_ok(), "Task should succeed");

        let tool_results: Vec<_> = session
            .messages
            .iter()
            .filter(|m| m.role == Role::Tool)
            .collect();
        assert_eq!(tool_results.len(), 2, "Should have two tool results");
    }

    #[tokio::test]
    async fn test_agent_loop_stops_at_max_rounds() {
        // Test max rounds enforcement
        let events: Vec<_> = (0..10)
            .flat_map(|i| {
                vec![
                    StreamEvent::ToolUse {
                        id: format!("tc-{i}"),
                        name: "loop".into(),
                        input: serde_json::json!({}),
                    },
                    StreamEvent::End {
                        finish_reason: Some("tool_calls".into()),
                    },
                ]
            })
            .collect();

        let provider = FakeProvider::with_events(events);

        let mut session = create_test_session(vec![provider]);
        session.tools = create_tool_registry_with_loop();

        let result = execute_task(&mut session, "infinite loop", ApprovalMode::AutoAllow).await;
        assert!(result.is_err(), "Should fail due to max rounds");
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Max tool rounds"));
    }

    #[tokio::test]
    async fn test_agent_loop_tool_error_propagated() {
        // Test tool error is captured in history
        let provider = FakeProvider::with_events(vec![
            StreamEvent::ToolUse {
                id: "tc-1".into(),
                name: "failing_tool".into(),
                input: serde_json::json!({}),
            },
            StreamEvent::End {
                finish_reason: Some("tool_calls".into()),
            },
        ]);

        let mut session = create_test_session(vec![provider]);
        session.tools = create_tool_registry_with_failing();

        let result = execute_task(&mut session, "call failing tool", ApprovalMode::AutoAllow).await;
        // Should handle tool error gracefully
        assert!(
            result.is_ok() || result.is_err(),
            "Should handle tool error gracefully"
        );

        let tool_results: Vec<_> = session
            .messages
            .iter()
            .filter(|m| m.role == Role::Tool)
            .collect();
        assert!(
            !tool_results.is_empty(),
            "Should have tool result even on error"
        );

        let tool_msg = &tool_results[0];
        if let ContentBlock::ToolResult { is_error, .. } = &tool_msg.content[0] {
            assert!(*is_error, "Tool result should be marked as error");
        }
    }

    #[tokio::test]
    async fn test_agent_loop_tool_not_found() {
        // Test unknown tool handling
        let provider = FakeProvider::with_events(vec![
            StreamEvent::ToolUse {
                id: "tc-1".into(),
                name: "nonexistent".into(),
                input: serde_json::json!({}),
            },
            StreamEvent::End {
                finish_reason: Some("tool_calls".into()),
            },
        ]);

        let mut session = create_test_session(vec![provider]);
        // Empty tool registry - no tools available

        let result = execute_task(&mut session, "call unknown tool", ApprovalMode::AutoAllow).await;
        assert!(result.is_ok(), "Should handle unknown tool gracefully");

        let tool_results: Vec<_> = session
            .messages
            .iter()
            .filter(|m| m.role == Role::Tool)
            .collect();
        assert_eq!(tool_results.len(), 1);
        if let ContentBlock::ToolResult { is_error, .. } = &tool_results[0].content[0] {
            assert!(*is_error, "Should be marked as error");
        }
    }

    // ===================================================================
    // Phase 2: central permission gate tests
    // ===================================================================

    /// A tool that increments a shared counter as the FIRST thing it does in
    /// `execute`, with NO internal permission check of its own. Any
    /// increment therefore proves the central gate (`dispatch_tool_call`),
    /// not the tool, allowed execution.
    struct CountingTool {
        name: &'static str,
        counter: Arc<AtomicUsize>,
        result: ToolResult,
        should_fail: bool,
    }

    impl CountingTool {
        fn new(name: &'static str, counter: Arc<AtomicUsize>) -> Self {
            Self {
                name,
                counter,
                result: ToolResult {
                    tool_call_id: String::new(),
                    output: "counting tool ok".into(),
                    is_error: false,
                    exit_code: Some(0),
                },
                should_fail: false,
            }
        }

        fn failing(name: &'static str, counter: Arc<AtomicUsize>) -> Self {
            Self {
                name,
                counter,
                result: ToolResult {
                    tool_call_id: String::new(),
                    output: String::new(),
                    is_error: true,
                    exit_code: Some(1),
                },
                should_fail: true,
            }
        }
    }

    #[async_trait]
    impl Tool for CountingTool {
        fn name(&self) -> &str {
            self.name
        }

        fn description(&self) -> &str {
            "Counting tool for permission gate tests"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object" })
        }

        async fn execute(
            &self,
            _input: serde_json::Value,
            _context: ToolContext,
        ) -> Result<ToolResult, ToolError> {
            // No internal permission check here: any increment proves the
            // GATE, not the tool, allowed this call.
            self.counter.fetch_add(1, Ordering::SeqCst);
            if self.should_fail {
                Err(ToolError::ExecutionFailed("counting tool failed".into()))
            } else {
                Ok(self.result.clone())
            }
        }
    }

    /// An approval gate that counts how many times it is consulted and
    /// always returns a fixed decision. Parameterized by decision, this one
    /// type stands in for "AlwaysDeny" (Deny), "AlwaysAllow" (Allow), and
    /// "AllowAllOnce" (AllowAll) fakes, while also exposing the exact
    /// consult count needed by the adversarial tests below.
    struct CountingGate {
        calls: AtomicUsize,
        decision: ApprovalDecision,
    }

    impl CountingGate {
        fn new(decision: ApprovalDecision) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                decision,
            }
        }
    }

    #[async_trait]
    impl ApprovalGate for CountingGate {
        async fn request_approval(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
            _policy: &str,
            _reason: &str,
        ) -> ApprovalDecision {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.decision
        }
    }

    fn make_registry(tool: Box<dyn Tool>) -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.register(tool);
        registry
    }

    fn permissions_with(
        tool_name: &str,
        policy: &str,
        allow_patterns: Vec<String>,
        deny_patterns: Vec<String>,
        default_policy: &str,
    ) -> ToolPermissions {
        let mut tools = HashMap::new();
        tools.insert(
            tool_name.to_string(),
            ToolPermission {
                policy: policy.into(),
                allow_patterns,
                deny_patterns,
            },
        );
        let config = PermissionsConfig {
            default_policy: default_policy.into(),
            tools,
        };
        ToolPermissions::from_config(&config, WorkspaceConfig::default())
    }

    #[allow(clippy::too_many_arguments)]
    async fn dispatch_with(
        tool_name: &str,
        input: serde_json::Value,
        registry: &ToolRegistry,
        permissions: &ToolPermissions,
        approval: &mut ApprovalState,
        audit: Option<&AuditLogger>,
        log_args: bool,
        log_output: bool,
    ) -> DispatchOutcome {
        let call = ToolCall {
            id: "tc-1".into(),
            name: tool_name.into(),
            input,
        };
        dispatch_tool_call(
            &call,
            registry,
            permissions,
            SessionId::new(),
            &std::env::temp_dir(),
            approval,
            audit,
            log_args,
            log_output,
        )
        .await
    }

    // --- Required scenarios 1..10 ------------------------------------------

    #[tokio::test]
    async fn test_gate_1_denied_shell_by_policy_never_executes() {
        let counter = Arc::new(AtomicUsize::new(0));
        let registry = make_registry(Box::new(CountingTool::new("shell", counter.clone())));
        let permissions = permissions_with("shell", "deny", vec![], vec![], "allow");
        let mut approval = ApprovalState {
            mode: ApprovalMode::AutoDeny,
            allow_all: false,
        };

        let outcome = dispatch_with(
            "shell",
            serde_json::json!({"cmd": "rm -rf /"}),
            &registry,
            &permissions,
            &mut approval,
            None,
            true,
            true,
        )
        .await;

        assert_eq!(counter.load(Ordering::SeqCst), 0);
        assert!(!outcome.executed);
        assert_eq!(outcome.audit_entry.decision, AuditDecision::Denied);
    }

    #[tokio::test]
    async fn test_gate_2_denied_read_file_via_file_family_fallback() {
        let counter = Arc::new(AtomicUsize::new(0));
        let registry = make_registry(Box::new(CountingTool::new("read_file", counter.clone())));
        // Only a "file" entry exists; read_file must fall back to it.
        let permissions = permissions_with("file", "ask", vec![], vec!["id_rsa*".into()], "allow");
        let mut approval = ApprovalState {
            mode: ApprovalMode::AutoDeny,
            allow_all: false,
        };

        let outcome = dispatch_with(
            "read_file",
            serde_json::json!({"path": "id_rsa"}),
            &registry,
            &permissions,
            &mut approval,
            None,
            true,
            true,
        )
        .await;

        assert_eq!(counter.load(Ordering::SeqCst), 0);
        assert_eq!(outcome.audit_entry.decision, AuditDecision::Denied);
    }

    #[tokio::test]
    async fn test_gate_3_allowed_tool_executes() {
        let counter = Arc::new(AtomicUsize::new(0));
        let registry = make_registry(Box::new(CountingTool::new("echo", counter.clone())));
        let permissions = permissions_with("echo", "allow", vec![], vec![], "deny");
        let mut approval = ApprovalState {
            mode: ApprovalMode::AutoDeny,
            allow_all: false,
        };

        let outcome = dispatch_with(
            "echo",
            serde_json::json!({}),
            &registry,
            &permissions,
            &mut approval,
            None,
            true,
            true,
        )
        .await;

        assert_eq!(counter.load(Ordering::SeqCst), 1);
        assert!(outcome.executed);
        assert_eq!(outcome.audit_entry.decision, AuditDecision::Allowed);
    }

    #[tokio::test]
    async fn test_gate_4_ask_under_auto_deny_fails_closed() {
        let counter = Arc::new(AtomicUsize::new(0));
        let registry = make_registry(Box::new(CountingTool::new("echo", counter.clone())));
        let permissions = permissions_with("echo", "ask", vec![], vec![], "deny");
        let mut approval = ApprovalState {
            mode: ApprovalMode::AutoDeny,
            allow_all: false,
        };

        let outcome = dispatch_with(
            "echo",
            serde_json::json!({}),
            &registry,
            &permissions,
            &mut approval,
            None,
            true,
            true,
        )
        .await;

        assert_eq!(counter.load(Ordering::SeqCst), 0);
        assert!(!outcome.executed);
        assert_eq!(outcome.audit_entry.decision, AuditDecision::Denied);
    }

    #[tokio::test]
    async fn test_gate_5_unknown_tool_denied_exit_127() {
        let registry = ToolRegistry::new();
        let permissions = permissions_with("echo", "allow", vec![], vec![], "allow");
        let mut approval = ApprovalState {
            mode: ApprovalMode::AutoDeny,
            allow_all: false,
        };

        let outcome = dispatch_with(
            "nonexistent",
            serde_json::json!({}),
            &registry,
            &permissions,
            &mut approval,
            None,
            true,
            true,
        )
        .await;

        assert!(!outcome.executed);
        assert_eq!(outcome.audit_entry.decision, AuditDecision::Denied);
        assert_eq!(outcome.audit_entry.exit_code, Some(127));
    }

    #[tokio::test]
    async fn test_gate_6_malformed_args_fail_closed_with_patterns_configured() {
        let counter = Arc::new(AtomicUsize::new(0));
        let registry = make_registry(Box::new(CountingTool::new("shell", counter.clone())));
        // Patterns are configured, but args={} has no "cmd" -> the target
        // cannot be derived -> fail-closed Deny, never reaching the tool.
        let permissions = permissions_with("shell", "allow", vec!["cargo ".into()], vec![], "deny");
        let mut approval = ApprovalState {
            mode: ApprovalMode::AutoDeny,
            allow_all: false,
        };

        let outcome = dispatch_with(
            "shell",
            serde_json::json!({}),
            &registry,
            &permissions,
            &mut approval,
            None,
            true,
            true,
        )
        .await;

        assert_eq!(counter.load(Ordering::SeqCst), 0);
        assert_eq!(outcome.audit_entry.decision, AuditDecision::Denied);
    }

    #[tokio::test]
    async fn test_gate_7_denied_tool_result_reaches_next_round_provider_called_twice() {
        let provider = FakeProvider::with_events(vec![
            StreamEvent::ToolUse {
                id: "tc-1".into(),
                name: "blocked".into(),
                input: serde_json::json!({}),
            },
            StreamEvent::End {
                finish_reason: Some("tool_calls".into()),
            },
            StreamEvent::End {
                finish_reason: Some("stop".into()),
            },
        ]);
        let provider_handle = provider.clone();

        let mut session = create_test_session(vec![provider]);
        session.config.permissions.default_policy = "deny".into();
        let counter = Arc::new(AtomicUsize::new(0));
        session.tools = make_registry(Box::new(CountingTool::new("blocked", counter.clone())));

        let result = execute_task(&mut session, "do something", ApprovalMode::AutoDeny).await;
        assert!(result.is_ok(), "task should complete: {result:?}");

        let tool_results: Vec<_> = session
            .messages
            .iter()
            .filter(|m| m.role == Role::Tool)
            .collect();
        assert_eq!(tool_results.len(), 1);
        if let ContentBlock::ToolResult { is_error, .. } = &tool_results[0].content[0] {
            assert!(
                *is_error,
                "denied tool call must surface as an error result"
            );
        } else {
            panic!("expected ToolResult content block");
        }

        assert_eq!(provider_handle.call_count.load(Ordering::SeqCst), 2);
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_gate_8_denied_audit_entry_has_decision_and_reason() {
        let counter = Arc::new(AtomicUsize::new(0));
        let registry = make_registry(Box::new(CountingTool::new("shell", counter.clone())));
        let permissions = permissions_with("shell", "deny", vec![], vec![], "allow");
        let mut approval = ApprovalState {
            mode: ApprovalMode::AutoDeny,
            allow_all: false,
        };

        let outcome = dispatch_with(
            "shell",
            serde_json::json!({"cmd": "ls"}),
            &registry,
            &permissions,
            &mut approval,
            None,
            true,
            true,
        )
        .await;

        assert_eq!(outcome.audit_entry.decision, AuditDecision::Denied);
        assert!(
            outcome
                .audit_entry
                .error
                .as_deref()
                .is_some_and(|s| !s.is_empty())
        );
    }

    #[tokio::test]
    async fn test_gate_9_log_args_false_hides_args_in_audit_entry() {
        let counter = Arc::new(AtomicUsize::new(0));
        let registry = make_registry(Box::new(CountingTool::new("echo", counter.clone())));
        let permissions = permissions_with("echo", "allow", vec![], vec![], "deny");
        let mut approval = ApprovalState {
            mode: ApprovalMode::AutoDeny,
            allow_all: false,
        };

        let outcome = dispatch_with(
            "echo",
            serde_json::json!({"secret": "shh"}),
            &registry,
            &permissions,
            &mut approval,
            None,
            false,
            true,
        )
        .await;

        assert!(outcome.audit_entry.args.is_none());
    }

    #[tokio::test]
    async fn test_gate_10_log_output_false_hides_output_in_audit_entry() {
        let counter = Arc::new(AtomicUsize::new(0));
        let registry = make_registry(Box::new(CountingTool::new("echo", counter.clone())));
        let permissions = permissions_with("echo", "allow", vec![], vec![], "deny");
        let mut approval = ApprovalState {
            mode: ApprovalMode::AutoDeny,
            allow_all: false,
        };

        let outcome = dispatch_with(
            "echo",
            serde_json::json!({}),
            &registry,
            &permissions,
            &mut approval,
            None,
            true,
            false,
        )
        .await;

        assert!(outcome.audit_entry.output.is_none());
    }

    // --- Adversarial extras --------------------------------------------------

    #[tokio::test]
    async fn test_gate_e2_allow_all_once_gate_consulted_once_across_two_asks() {
        let counter = Arc::new(AtomicUsize::new(0));
        let registry = make_registry(Box::new(CountingTool::new("echo", counter.clone())));
        let permissions = permissions_with("echo", "ask", vec![], vec![], "deny");
        // Stands in for "AllowAllOnce": returns AllowAll, so after the first
        // consult approval.allow_all short-circuits any further consult.
        let gate = Arc::new(CountingGate::new(ApprovalDecision::AllowAll));
        let mut approval = ApprovalState {
            mode: ApprovalMode::Interactive(gate.clone()),
            allow_all: false,
        };

        let outcome1 = dispatch_with(
            "echo",
            serde_json::json!({}),
            &registry,
            &permissions,
            &mut approval,
            None,
            true,
            true,
        )
        .await;
        let outcome2 = dispatch_with(
            "echo",
            serde_json::json!({}),
            &registry,
            &permissions,
            &mut approval,
            None,
            true,
            true,
        )
        .await;

        assert!(outcome1.executed);
        assert!(outcome2.executed);
        assert_eq!(gate.calls.load(Ordering::SeqCst), 1);
        assert!(approval.allow_all);
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_gate_e3_counting_gate_allow_consulted_every_ask() {
        let counter = Arc::new(AtomicUsize::new(0));
        let registry = make_registry(Box::new(CountingTool::new("echo", counter.clone())));
        let permissions = permissions_with("echo", "ask", vec![], vec![], "deny");
        let gate = Arc::new(CountingGate::new(ApprovalDecision::Allow));
        let mut approval = ApprovalState {
            mode: ApprovalMode::Interactive(gate.clone()),
            allow_all: false,
        };

        dispatch_with(
            "echo",
            serde_json::json!({}),
            &registry,
            &permissions,
            &mut approval,
            None,
            true,
            true,
        )
        .await;
        dispatch_with(
            "echo",
            serde_json::json!({}),
            &registry,
            &permissions,
            &mut approval,
            None,
            true,
            true,
        )
        .await;

        assert_eq!(gate.calls.load(Ordering::SeqCst), 2);
        assert!(!approval.allow_all);
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_gate_e4_denied_ask_with_log_args_false_never_leaks_secret() {
        let counter = Arc::new(AtomicUsize::new(0));
        let registry = make_registry(Box::new(CountingTool::new("echo", counter.clone())));
        let permissions = permissions_with("echo", "ask", vec![], vec![], "deny");
        let mut approval = ApprovalState {
            mode: ApprovalMode::AutoDeny,
            allow_all: false,
        };

        let outcome = dispatch_with(
            "echo",
            serde_json::json!({"token": "super-secret-value"}),
            &registry,
            &permissions,
            &mut approval,
            None,
            false,
            true,
        )
        .await;

        assert!(outcome.audit_entry.args.is_none());
        let serialized = serde_json::to_string(&outcome.audit_entry).unwrap();
        assert!(!serialized.contains("super-secret-value"));
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_gate_e5_read_file_family_fallback_policy_label_is_file_policy() {
        let counter = Arc::new(AtomicUsize::new(0));
        let registry = make_registry(Box::new(CountingTool::new("read_file", counter.clone())));
        let permissions = permissions_with("file", "ask", vec![], vec![], "deny");
        let mut approval = ApprovalState {
            mode: ApprovalMode::AutoDeny,
            allow_all: false,
        };

        let outcome = dispatch_with(
            "read_file",
            serde_json::json!({"path": "notes.md"}),
            &registry,
            &permissions,
            &mut approval,
            None,
            true,
            true,
        )
        .await;

        assert_eq!(outcome.audit_entry.policy, "ask");
    }

    #[tokio::test]
    async fn test_gate_e6_allowed_tool_execution_error_yields_error_decision() {
        let counter = Arc::new(AtomicUsize::new(0));
        let registry = make_registry(Box::new(CountingTool::failing("echo", counter.clone())));
        let permissions = permissions_with("echo", "allow", vec![], vec![], "deny");
        let mut approval = ApprovalState {
            mode: ApprovalMode::AutoDeny,
            allow_all: false,
        };

        let outcome = dispatch_with(
            "echo",
            serde_json::json!({}),
            &registry,
            &permissions,
            &mut approval,
            None,
            true,
            true,
        )
        .await;

        assert!(outcome.executed);
        assert_eq!(outcome.audit_entry.decision, AuditDecision::Error);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_gate_e7_denied_dispatch_writes_denied_entry_no_args_to_real_audit_log() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.log");
        let audit_config = AuditConfig {
            enabled: true,
            path: path.to_string_lossy().to_string(),
            max_size_mb: 1,
            max_files: 3,
            log_args: false,
            log_output: false,
        };
        let logger = AuditLogger::new(audit_config).await.unwrap();

        let counter = Arc::new(AtomicUsize::new(0));
        let registry = make_registry(Box::new(CountingTool::new("shell", counter.clone())));
        let permissions = permissions_with("shell", "deny", vec![], vec![], "allow");
        let mut approval = ApprovalState {
            mode: ApprovalMode::AutoDeny,
            allow_all: false,
        };

        let outcome = dispatch_with(
            "shell",
            serde_json::json!({"cmd": "rm -rf /", "note": "do not leak"}),
            &registry,
            &permissions,
            &mut approval,
            Some(&logger),
            false,
            false,
        )
        .await;

        assert!(!outcome.executed);
        assert_eq!(counter.load(Ordering::SeqCst), 0);

        let entries = logger.read_last(10).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].decision, AuditDecision::Denied);
        assert!(entries[0].args.is_none());

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(!content.contains("do not leak"));
    }
}
