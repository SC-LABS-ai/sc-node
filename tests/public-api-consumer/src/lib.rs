//! Compile-time consumer contract for the intentional SC Node alpha API.

#![cfg(test)]

use async_trait::async_trait;
use futures::stream;
use sc_message_types::{
    CompletionRequest, Message, ModelInfo, ProviderInfo, SessionId, StreamEvent, ToolDefinition,
    ToolResult,
};
use sc_provider_core::routing::{
    AvailableProvider, RouteReason, RouteRequest, RoutingConfig, resolve_route,
};
use sc_provider_core::{EventStream, Provider, Result as ProviderResult};
use sc_tool_core::{
    ApprovalDecision, ApprovalGate, PermissionDecision, Tool, ToolContext, ToolError, ToolRegistry,
    check_permission,
};

struct FixtureProvider;

#[async_trait]
impl Provider for FixtureProvider {
    fn key(&self) -> &str {
        "fixture"
    }

    fn name(&self) -> &str {
        "Fixture Provider"
    }

    async fn list_models(&self) -> ProviderResult<Vec<ModelInfo>> {
        Ok(vec![ModelInfo {
            id: "fixture-model".into(),
            name: "Fixture Model".into(),
            context_window: 4096,
            supports_tools: true,
            supports_streaming: true,
        }])
    }

    async fn complete(&self, _request: CompletionRequest) -> ProviderResult<EventStream> {
        let events: Vec<ProviderResult<StreamEvent>> = vec![
            Ok(StreamEvent::TextDelta { text: "ok".into() }),
            Ok(StreamEvent::End {
                finish_reason: Some("stop".into()),
            }),
        ];
        Ok(Box::pin(stream::iter(events)))
    }
}

struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }

    fn description(&self) -> &str {
        "Echo a value"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"value":{"type":"string"}}})
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        _context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult {
            tool_call_id: "fixture-call".into(),
            output: input.to_string(),
            is_error: false,
            exit_code: Some(0),
        })
    }
}

struct DenyGate;

#[async_trait]
impl ApprovalGate for DenyGate {
    async fn request_approval(
        &self,
        _tool_name: &str,
        _args: &serde_json::Value,
        _policy: &str,
        _reason: &str,
    ) -> ApprovalDecision {
        ApprovalDecision::Deny
    }
}

#[test]
fn message_routing_and_registry_surface_compiles() {
    let session = SessionId::new();
    let reparsed: SessionId = session.to_string().parse().expect("session id roundtrip");
    assert_eq!(session, reparsed);

    let message = Message::user("hello");
    let request = CompletionRequest {
        model: "fixture-model".into(),
        messages: vec![message],
        tools: vec![ToolDefinition {
            name: "echo".into(),
            description: "Echo a value".into(),
            parameters: serde_json::json!({"type":"object"}),
        }],
        system: None,
        stream: true,
        temperature: Some(0.0),
        max_tokens: Some(16),
    };
    assert_eq!(request.model, "fixture-model");

    let providers = vec![AvailableProvider::new("fixture", true, true, true)];
    let route = resolve_route(
        &providers,
        &RoutingConfig::default(),
        &RouteRequest::default(),
        false,
    )
    .expect("local route");
    assert_eq!(route.reason, RouteReason::LocalFirst);

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(EchoTool));
    assert!(registry.get("echo").is_some());
    assert_eq!(registry.definitions().len(), 1);

    let permissions = sc_tool_core::ToolPermissions::from_config(
        &sc_config::PermissionsConfig::default(),
        sc_config::WorkspaceConfig::default(),
    );
    let decision = check_permission("unknown", &serde_json::json!({}), &permissions);
    assert!(matches!(
        decision,
        PermissionDecision::Ask(_) | PermissionDecision::Deny(_)
    ));
}

#[tokio::test]
async fn provider_and_approval_traits_are_external_implementable() {
    let provider = FixtureProvider;
    let info: ProviderInfo = provider.info();
    assert_eq!(info.key, "fixture");
    assert_eq!(provider.list_models().await.expect("models").len(), 1);

    let request = CompletionRequest {
        model: "fixture-model".into(),
        messages: vec![Message::user("hello")],
        tools: vec![],
        system: None,
        stream: true,
        temperature: None,
        max_tokens: None,
    };
    let _stream = provider.complete(request).await.expect("completion stream");

    let decision = DenyGate
        .request_approval("echo", &serde_json::json!({}), "ask", "fixture")
        .await;
    assert_eq!(decision, ApprovalDecision::Deny);
}

#[test]
fn contract_and_proof_surface_compiles() {
    let contract = sc_contract::ExecutionContract::parse(
        r#"
schema_version = 1
task_id = "public-api"
task = "Compile external consumer"
worker = "fixture"
workspace = "/tmp/sc-node"
"#,
    )
    .expect("contract parse");
    contract.validate().expect("contract validate");
    let policy_hash = contract.policy_hash().expect("policy hash");
    assert_eq!(policy_hash.len(), 64);
    assert!(contract.explain().contains("public-api"));

    let plan = sc_contract::preflight::ProposedPlan::default();
    let _report = sc_contract::preflight::preflight(&plan, &contract);

    let now = chrono::Utc::now();
    let event = sc_proof::AuditEvent::new(
        0,
        now,
        "fixture",
        "external consumer",
        serde_json::json!({"token":"secret-value"}),
    );
    let chain = sc_proof::build_chain(vec![event.clone()]).expect("chain");
    assert!(sc_proof::chain_head(&chain).is_some());

    let bundle = sc_proof::ProofBundle::new(
        "public-api",
        policy_hash,
        "fixture",
        "fixture",
        "fixture-model",
        now,
        now,
    )
    .with_audit_events(vec![event])
    .expect("audit chain")
    .with_expected_event_count(1)
    .with_checks(vec![sc_proof::CheckOutcome::new(
        "consumer",
        "cargo test -p sc-public-api-consumer",
        true,
        "passed",
    )]);
    sc_proof::verify(&bundle).expect("proof verify");
    sc_proof::check_event_count(&bundle).expect("event count");
    assert!(
        bundle
            .canonical_json()
            .expect("canonical json")
            .contains("public-api")
    );
}
