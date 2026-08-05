use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use chrono::{Duration as ChronoDuration, Utc};
use clap::{Parser, Subcommand};
use sc_agent_core::{ApprovalMode, Session, run_session};
use sc_audit::{AuditLogger, create_audit_entry};
use sc_config::{AuditConfig, Config, OllamaConfig, ToolPermission};
use sc_message_types::{
    AuditDecision, CompletionRequest, ModelInfo, ProviderInfo, SessionId, StreamEvent, ToolResult,
};
use sc_proof::{AuditEvent, ProofBundle, build_chain, verify};
use sc_provider_core::routing::{
    AvailableProvider, FallbackRoute, RouteRequest, RoutingConfig as CoreRoutingConfig,
    RoutingRule as CoreRoutingRule, resolve_route,
};
use sc_provider_core::{EventStream, Provider};
use sc_provider_ollama::OllamaProvider;
use sc_tool_core::{
    PermissionDecision, Tool, ToolContext, ToolError, ToolPermissions, ToolRegistry,
    check_permission,
};
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Parser, Debug)]
#[command(name = "sc-benchmark", about = "Reproducible SC Node benchmark helper")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Call Ollama directly with the same no-tool request shape used by SC Node.
    DirectOllama(OllamaArgs),
    /// Run the request through SC Node routing, session construction, and provider adapter.
    NodeOllama(OllamaArgs),
    /// Ask Ollama to unload a model so the next invocation is a cold-model run.
    UnloadOllama {
        #[arg(long)]
        model: String,
        #[arg(long, default_value = "http://127.0.0.1:11434")]
        base_url: String,
    },
    /// Run deterministic in-process microbenchmarks with no model or network latency.
    Micro {
        #[arg(long, default_value_t = 100_000)]
        iterations: u64,
        #[arg(long, default_value_t = 1_000)]
        io_iterations: u64,
    },
    /// Run a synthetic no-model agent loop with or without one no-op tool round.
    SyntheticAgent {
        #[arg(long, default_value_t = 20)]
        iterations: u64,
        #[arg(long)]
        tool_round: bool,
        #[arg(long)]
        audit: bool,
    },
    /// Serve a deterministic local Ollama-compatible fixture endpoint.
    ServeMockOllama {
        #[arg(long, default_value = "127.0.0.1:11555")]
        bind: String,
    },
}

#[derive(clap::Args, Debug, Clone)]
struct OllamaArgs {
    #[arg(long)]
    model: String,
    #[arg(long, default_value = "Reply exactly with OK.")]
    prompt: String,
    #[arg(long, default_value = "http://127.0.0.1:11434")]
    base_url: String,
    #[arg(long, default_value = "5m")]
    keep_alive: String,
    #[arg(long, default_value_t = 120)]
    timeout_secs: u64,
}

#[derive(Debug, Serialize)]
struct CommandResult {
    mode: &'static str,
    model: String,
    elapsed_ms: f64,
    response_chars: usize,
    response: String,
}

#[derive(Debug, Serialize)]
struct MicroResult {
    benchmark: &'static str,
    iterations: u64,
    total_ns: u128,
    ns_per_operation: f64,
}

#[derive(Debug, Serialize)]
struct SyntheticResult {
    mode: &'static str,
    iterations: u64,
    audit: bool,
    tool_executions: usize,
    total_ns: u128,
    ns_per_iteration: f64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::DirectOllama(args) => direct_ollama(args).await,
        Command::NodeOllama(args) => node_ollama(args).await,
        Command::UnloadOllama { model, base_url } => unload_ollama(&base_url, &model).await,
        Command::Micro {
            iterations,
            io_iterations,
        } => micro(iterations, io_iterations).await,
        Command::SyntheticAgent {
            iterations,
            tool_round,
            audit,
        } => synthetic_agent(iterations, tool_round, audit).await,
        Command::ServeMockOllama { bind } => serve_mock_ollama(&bind).await,
    }
}

async fn serve_mock_ollama(bind: &str) -> Result<()> {
    let listener = TcpListener::bind(bind)
        .await
        .with_context(|| format!("failed to bind mock Ollama endpoint at {bind}"))?;
    println!("MOCK_OLLAMA_READY={bind}");
    loop {
        let (stream, _) = listener.accept().await?;
        tokio::spawn(async move {
            if let Err(error) = handle_mock_connection(stream).await {
                eprintln!("mock Ollama connection failed: {error:#}");
            }
        });
    }
}

async fn handle_mock_connection(mut stream: TcpStream) -> Result<()> {
    let mut request = Vec::with_capacity(4096);
    let mut chunk = [0u8; 2048];
    let mut header_end = None;
    let mut content_length = 0usize;

    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..read]);
        if header_end.is_none()
            && let Some(position) = request.windows(4).position(|window| window == b"\r\n\r\n")
        {
            let end = position + 4;
            header_end = Some(end);
            let headers = String::from_utf8_lossy(&request[..end]);
            content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
        }
        if let Some(end) = header_end
            && request.len() >= end + content_length
        {
            break;
        }
        if request.len() > 1_048_576 {
            return Err(anyhow!("mock request exceeded 1 MiB"));
        }
    }

    let request_text = String::from_utf8_lossy(&request);
    let request_line = request_text.lines().next().unwrap_or_default();
    let path = request_line.split_whitespace().nth(1).unwrap_or("/");
    let (content_type, body, status) = match path {
        "/api/chat" => (
            "application/x-ndjson",
            concat!(
                "{\"model\":\"fixture\",\"message\":{\"role\":\"assistant\",\"content\":\"OK\"},\"done\":false}\n",
                "{\"model\":\"fixture\",\"message\":{\"role\":\"assistant\",\"content\":\"\"},\"done\":true,\"done_reason\":\"stop\"}\n"
            )
            .to_string(),
            "200 OK",
        ),
        "/api/tags" => (
            "application/json",
            "{\"models\":[{\"name\":\"fixture\",\"size\":0,\"details\":{\"family\":\"fixture\"}}]}".to_string(),
            "200 OK",
        ),
        "/api/generate" => (
            "application/json",
            "{\"model\":\"fixture\",\"response\":\"\",\"done\":true}".to_string(),
            "200 OK",
        ),
        _ => (
            "application/json",
            "{\"error\":\"not found\"}".to_string(),
            "404 Not Found",
        ),
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await?;
    Ok(())
}

async fn direct_ollama(args: OllamaArgs) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(args.timeout_secs))
        .build()?;
    let url = format!("{}/api/chat", args.base_url.trim_end_matches('/'));
    let body = json!({
        "model": args.model,
        "messages": [{"role": "user", "content": args.prompt}],
        "stream": true,
        "options": {"temperature": 0.7, "num_predict": 4096},
        "keep_alive": args.keep_alive,
    });

    let started = Instant::now();
    let response = client.post(url).json(&body).send().await?;
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        return Err(anyhow!("Ollama returned HTTP {status}: {text}"));
    }
    let response_text = collect_ollama_text(&text)?;
    let result = CommandResult {
        mode: "direct_ollama",
        model: args.model,
        elapsed_ms: started.elapsed().as_secs_f64() * 1_000.0,
        response_chars: response_text.chars().count(),
        response: response_text,
    };
    println!("BENCH_RESULT={}", serde_json::to_string(&result)?);
    Ok(())
}

fn collect_ollama_text(body: &str) -> Result<String> {
    let mut output = String::new();
    for line in body.lines().filter(|line| !line.trim().is_empty()) {
        let value: serde_json::Value = serde_json::from_str(line)
            .with_context(|| format!("invalid Ollama JSON line: {line}"))?;
        if let Some(error) = value.get("error").and_then(|v| v.as_str()) {
            return Err(anyhow!("Ollama error: {error}"));
        }
        if let Some(content) = value
            .get("message")
            .and_then(|v| v.get("content"))
            .and_then(|v| v.as_str())
        {
            output.push_str(content);
        }
    }
    Ok(output)
}

async fn node_ollama(args: OllamaArgs) -> Result<()> {
    let ollama = OllamaConfig {
        enabled: true,
        base_url: args.base_url,
        default_model: args.model.clone(),
        keep_alive: args.keep_alive,
        timeout_secs: args.timeout_secs,
        ..OllamaConfig::default()
    };

    let provider: Arc<dyn Provider> = Arc::new(OllamaProvider::new(ollama.clone())?);
    let mut config = Config::default();
    config.providers.ollama = Some(ollama);
    if let Some(openrouter) = config.providers.openrouter.as_mut() {
        openrouter.enabled = false;
    }
    if let Some(nvidia) = config.providers.nvidia.as_mut() {
        nvidia.enabled = false;
    }
    config.routing.rules.clear();
    config.routing.fallback_provider = "ollama".to_string();
    config.routing.fallback_model = args.model.clone();
    config.audit.enabled = false;

    let session = Session::new(config, vec![provider], ToolRegistry::new(), None);
    let started = Instant::now();
    run_session(session, Some(args.prompt), ApprovalMode::AutoDeny).await?;
    let result = CommandResult {
        mode: "node_ollama",
        model: args.model,
        elapsed_ms: started.elapsed().as_secs_f64() * 1_000.0,
        response_chars: 0,
        response: "SC Node writes streamed output before this structured marker".to_string(),
    };
    println!("BENCH_RESULT={}", serde_json::to_string(&result)?);
    Ok(())
}

async fn unload_ollama(base_url: &str, model: &str) -> Result<()> {
    let client = reqwest::Client::new();
    let url = format!("{}/api/generate", base_url.trim_end_matches('/'));
    let response = client
        .post(url)
        .json(&json!({
            "model": model,
            "prompt": "",
            "stream": false,
            "keep_alive": 0,
        }))
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        return Err(anyhow!("Ollama unload returned HTTP {status}: {body}"));
    }
    println!("UNLOADED={model}");
    Ok(())
}

fn measure_sync<F>(name: &'static str, iterations: u64, mut operation: F) -> MicroResult
where
    F: FnMut(),
{
    let started = Instant::now();
    for _ in 0..iterations {
        operation();
    }
    let total_ns = started.elapsed().as_nanos();
    MicroResult {
        benchmark: name,
        iterations,
        total_ns,
        ns_per_operation: total_ns as f64 / iterations.max(1) as f64,
    }
}

async fn micro(iterations: u64, io_iterations: u64) -> Result<()> {
    let providers = vec![
        AvailableProvider::new("ollama", true, true, true),
        AvailableProvider::new("nvidia", true, true, false),
    ];
    let routing = CoreRoutingConfig {
        rules: vec![CoreRoutingRule {
            name: "code".to_string(),
            match_contains: vec!["rust".to_string(), "cargo".to_string()],
            provider: "ollama".to_string(),
            model: "qwen:latest".to_string(),
        }],
        fallback: Some(FallbackRoute {
            provider: "ollama".to_string(),
            model: "qwen:latest".to_string(),
            enabled: true,
        }),
    };
    let request = RouteRequest {
        requested_provider: None,
        requested_model: None,
        task: Some("review this rust module".to_string()),
    };
    let routing_result = measure_sync("routing.resolve_route", iterations, || {
        std::hint::black_box(resolve_route(&providers, &routing, &request, false).unwrap());
    });

    let mut tools = HashMap::new();
    tools.insert(
        "shell".to_string(),
        ToolPermission {
            policy: "ask".to_string(),
            allow_patterns: vec!["cargo ".to_string()],
            deny_patterns: vec!["rm -rf".to_string()],
        },
    );
    let permission_config = sc_config::PermissionsConfig {
        default_policy: "deny".to_string(),
        tools,
    };
    let permissions =
        ToolPermissions::from_config(&permission_config, sc_config::WorkspaceConfig::default());
    let permission_args = json!({"cmd": ["cargo", "check", "--workspace"]});
    let permission_result = measure_sync("permission.check", iterations, || {
        let decision = check_permission("shell", &permission_args, &permissions);
        assert!(matches!(decision, PermissionDecision::Ask(_)));
        std::hint::black_box(decision);
    });

    let disabled_dir = tempfile::tempdir()?;
    let disabled_logger = AuditLogger::new(AuditConfig {
        enabled: false,
        path: disabled_dir
            .path()
            .join("disabled.jsonl")
            .display()
            .to_string(),
        ..AuditConfig::default()
    })
    .await?;
    let started = Instant::now();
    for _ in 0..io_iterations {
        disabled_logger
            .log(create_audit_entry(
                SessionId::new(),
                "noop",
                None,
                "allow",
                AuditDecision::Allowed,
                Some(0),
                0,
                None,
                false,
                false,
                None,
            ))
            .await?;
    }
    let total_ns = started.elapsed().as_nanos();
    let audit_disabled = MicroResult {
        benchmark: "audit.log_disabled",
        iterations: io_iterations,
        total_ns,
        ns_per_operation: total_ns as f64 / io_iterations.max(1) as f64,
    };

    let enabled_dir = tempfile::tempdir()?;
    let enabled_logger = AuditLogger::new(AuditConfig {
        enabled: true,
        path: enabled_dir
            .path()
            .join("enabled.jsonl")
            .display()
            .to_string(),
        max_size_mb: 100,
        max_files: 2,
        log_args: false,
        log_output: false,
    })
    .await?;
    let started = Instant::now();
    for _ in 0..io_iterations {
        enabled_logger
            .log(create_audit_entry(
                SessionId::new(),
                "noop",
                None,
                "allow",
                AuditDecision::Allowed,
                Some(0),
                0,
                None,
                false,
                false,
                None,
            ))
            .await?;
    }
    let total_ns = started.elapsed().as_nanos();
    let audit_enabled = MicroResult {
        benchmark: "audit.log_enabled_flush_each",
        iterations: io_iterations,
        total_ns,
        ns_per_operation: total_ns as f64 / io_iterations.max(1) as f64,
    };

    let proof_events: Vec<AuditEvent> = (0..100)
        .map(|index| {
            AuditEvent::new(
                index,
                Utc::now() + ChronoDuration::milliseconds(index as i64),
                "tool_call",
                "benchmark fixture",
                json!({"tool": "noop", "index": index}),
            )
        })
        .collect();
    let proof_iterations = io_iterations.min(10_000);
    let proof_build = measure_sync("proof.build_chain_100_events", proof_iterations, || {
        std::hint::black_box(build_chain(proof_events.clone()).unwrap());
    });
    let chain = build_chain(proof_events)?;
    let bundle = ProofBundle::new(
        "benchmark",
        "fixture",
        "sc-benchmark",
        "none",
        "none",
        Utc::now(),
        Utc::now(),
    )
    .with_audit_events(chain.iter().map(|item| item.event.clone()).collect())?;
    let proof_verify = measure_sync("proof.verify_100_events", proof_iterations, || {
        verify(std::hint::black_box(&bundle)).unwrap();
    });

    for result in [
        routing_result,
        permission_result,
        audit_disabled,
        audit_enabled,
        proof_build,
        proof_verify,
    ] {
        println!("MICRO_RESULT={}", serde_json::to_string(&result)?);
    }
    Ok(())
}

struct ScriptedProvider {
    with_tool: bool,
    calls: AtomicUsize,
}

impl ScriptedProvider {
    fn new(with_tool: bool) -> Arc<Self> {
        Arc::new(Self {
            with_tool,
            calls: AtomicUsize::new(0),
        })
    }
}

#[async_trait]
impl Provider for ScriptedProvider {
    fn key(&self) -> &str {
        "bench"
    }

    fn name(&self) -> &str {
        "Benchmark fixture provider"
    }

    async fn list_models(&self) -> sc_provider_core::Result<Vec<ModelInfo>> {
        Ok(Vec::new())
    }

    async fn complete(&self, _request: CompletionRequest) -> sc_provider_core::Result<EventStream> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let events = if self.with_tool && call == 0 {
            vec![
                Ok(StreamEvent::ToolUse {
                    id: "bench-call".to_string(),
                    name: "noop".to_string(),
                    input: json!({}),
                }),
                Ok(StreamEvent::End {
                    finish_reason: Some("tool_calls".to_string()),
                }),
            ]
        } else {
            vec![
                Ok(StreamEvent::TextDelta {
                    text: "OK".to_string(),
                }),
                Ok(StreamEvent::End {
                    finish_reason: Some("stop".to_string()),
                }),
            ]
        };
        Ok(Box::pin(futures::stream::iter(events)))
    }

    async fn health_check(&self) -> sc_provider_core::Result<bool> {
        Ok(true)
    }

    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            key: self.key().to_string(),
            name: self.name().to_string(),
            models: Vec::new(),
        }
    }
}

struct NoopTool {
    executions: Arc<AtomicUsize>,
}

#[async_trait]
impl Tool for NoopTool {
    fn name(&self) -> &str {
        "noop"
    }

    fn description(&self) -> &str {
        "Benchmark no-op tool"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({"type": "object", "additionalProperties": false})
    }

    async fn execute(
        &self,
        _input: serde_json::Value,
        _context: ToolContext,
    ) -> std::result::Result<ToolResult, ToolError> {
        self.executions.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult {
            tool_call_id: "bench-call".to_string(),
            output: "noop".to_string(),
            is_error: false,
            exit_code: Some(0),
        })
    }
}

async fn synthetic_agent(iterations: u64, tool_round: bool, audit: bool) -> Result<()> {
    let temp = tempfile::tempdir()?;
    let tool_executions = Arc::new(AtomicUsize::new(0));
    let started = Instant::now();
    for iteration in 0..iterations {
        let provider: Arc<dyn Provider> = ScriptedProvider::new(tool_round);
        let mut config = Config::default();
        config.providers.ollama = None;
        config.providers.openrouter = None;
        config.providers.nvidia = None;
        config.routing.rules.clear();
        config.routing.fallback_provider = "bench".to_string();
        config.routing.fallback_model = "fixture".to_string();
        config.permissions.default_policy = "deny".to_string();
        config.permissions.tools.insert(
            "noop".to_string(),
            ToolPermission {
                policy: "allow".to_string(),
                allow_patterns: Vec::new(),
                deny_patterns: Vec::new(),
            },
        );

        let mut registry = ToolRegistry::new();
        if tool_round {
            registry.register(Box::new(NoopTool {
                executions: tool_executions.clone(),
            }));
        }
        let audit_logger = if audit {
            let path: PathBuf = temp.path().join(format!("audit-{iteration}.jsonl"));
            let audit_config = AuditConfig {
                enabled: true,
                path: path.display().to_string(),
                max_size_mb: 100,
                max_files: 2,
                log_args: false,
                log_output: false,
            };
            config.audit = audit_config.clone();
            Some(Arc::new(AuditLogger::new(audit_config).await?))
        } else {
            config.audit.enabled = false;
            None
        };
        let session = Session::new(config, vec![provider], registry, audit_logger);
        run_session(
            session,
            Some("synthetic benchmark".to_string()),
            ApprovalMode::AutoDeny,
        )
        .await?;
    }
    let total_ns = started.elapsed().as_nanos();
    let observed_tool_executions = tool_executions.load(Ordering::SeqCst);
    let expected_tool_executions = if tool_round { iterations as usize } else { 0 };
    if observed_tool_executions != expected_tool_executions {
        return Err(anyhow!(
            "synthetic dispatch count mismatch: expected {expected_tool_executions}, observed {observed_tool_executions}"
        ));
    }
    let result = SyntheticResult {
        mode: if tool_round {
            "synthetic_tool_round"
        } else {
            "synthetic_no_tool"
        },
        iterations,
        audit,
        tool_executions: observed_tool_executions,
        total_ns,
        ns_per_iteration: total_ns as f64 / iterations.max(1) as f64,
    };
    println!("SYNTHETIC_RESULT={}", serde_json::to_string(&result)?);
    Ok(())
}
