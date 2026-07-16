//! Main entry point for SC Node.
//!
//! CLI parsing runs first — subcommands like --help, --version, and init
//! work without an existing config file. All other commands require
//! a valid config at ~/.sc-agent/config.toml.

use clap::Parser;
use sc_agent_core::{ApprovalMode, Cli, Commands, ContractCmd, ProofCmd, Session, run_session};
use sc_audit::AuditLogger;
use sc_config::Config;
use sc_provider_core::Provider;
use sc_provider_nvidia::NvidiaProvider;
use sc_provider_ollama::OllamaProvider;
use sc_provider_openrouter::OpenRouterProvider;
use sc_tool_core::{CliApprovalGate, ToolRegistry};
use sc_tool_file::{ListDirTool, ReadFileTool, WriteFileTool};
use sc_tool_shell::ShellTool;
use std::io::IsTerminal;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    // ── Commands that do NOT require config ──────────────────

    if let Commands::ConfigInit = &cli.command {
        let path = Config::create_default()?;
        println!("Created default config at: {}", path.display());
        return Ok(());
    }

    if let Commands::Contract { action } = &cli.command {
        return run_contract(action);
    }

    if let Commands::Proof { action } = &cli.command {
        return run_proof(action);
    }

    // ── Commands that DO require config ──────────────────────

    let config = match Config::load() {
        Ok(c) => c,
        Err(sc_config::ConfigError::NotFound(_)) => {
            eprintln!("No config found. Run 'sc-agent init' first.");
            eprintln!("Expected at: {}", Config::default_path()?.display());
            anyhow::bail!("Config not found");
        }
        Err(e) => return Err(e.into()),
    };

    // Audit logger
    let audit = AuditLogger::new(config.audit.clone())
        .await
        .ok()
        .map(Arc::new);

    // Providers
    let mut providers: Vec<Arc<dyn Provider>> = Vec::new();

    if let Some(openrouter_cfg) = &config.providers.openrouter
        && openrouter_cfg.enabled
    {
        match OpenRouterProvider::new(openrouter_cfg.clone()) {
            Ok(provider) => providers.push(Arc::new(provider)),
            Err(err) => eprintln!("OpenRouter provider disabled: {err}"),
        }
    }

    if let Some(nvidia_cfg) = &config.providers.nvidia
        && nvidia_cfg.enabled
    {
        match NvidiaProvider::new(nvidia_cfg.clone()) {
            Ok(provider) => providers.push(Arc::new(provider)),
            Err(err) => eprintln!("NVIDIA provider disabled: {err}"),
        }
    }

    if let Some(ollama_cfg) = &config.providers.ollama
        && ollama_cfg.enabled
    {
        match OllamaProvider::new(ollama_cfg.clone()) {
            Ok(provider) => providers.push(Arc::new(provider)),
            Err(err) => eprintln!("Ollama provider disabled: {err}"),
        }
    }

    // Tools
    let mut tools = ToolRegistry::new();
    tools.register(Box::new(ReadFileTool));
    tools.register(Box::new(WriteFileTool));
    tools.register(Box::new(ListDirTool));
    tools.register(Box::new(ShellTool));

    let session = Session::new(config, providers, tools, audit);

    // Approval mode: interactive prompting is only available for `repl` on a
    // real TTY. Every other case (including `run`, and `repl` fed via a
    // piped/redirected non-TTY stdin) fails closed — no human is present to
    // approve an `Ask` decision, so it must be denied rather than silently
    // allowed.
    let approval_mode = match &cli.command {
        Commands::Repl if std::io::stdin().is_terminal() => {
            ApprovalMode::Interactive(Arc::new(CliApprovalGate::new()))
        }
        _ => ApprovalMode::AutoDeny,
    };

    // ── Execute command ──────────────────────────────────────

    match cli.command {
        Commands::Run { task } => {
            run_session(session, Some(task), approval_mode).await?;
        }
        Commands::Repl => {
            run_session(session, None, approval_mode).await?;
        }
        Commands::ConfigShow => {
            println!("{}", toml::to_string_pretty(&session.config)?);
        }
        Commands::ConfigSet { key, value } => {
            eprintln!("Config set not yet implemented (set {key}={value})");
        }
        Commands::ProvidersList => {
            println!("Configured providers:");
            for p in &session.providers {
                println!("  - {} ({})", p.name(), p.key());
            }
            if session.providers.is_empty() {
                println!("  (none enabled)");
            }
        }
        Commands::ModelsList => {
            println!("Available models:");
            for provider in &session.providers {
                println!("\n{} ({}):", provider.name(), provider.key());
                match provider.list_models().await {
                    Ok(models) => {
                        if models.is_empty() {
                            println!("  (none listed)");
                        } else {
                            for m in models {
                                println!(
                                    "  - {} (ctx: {}, tools: {}, stream: {})",
                                    m.id, m.context_window, m.supports_tools, m.supports_streaming
                                );
                            }
                        }
                    }
                    Err(e) => println!("  Error: {e}"),
                }
            }
        }
        Commands::AuditShow { last } => {
            if let Some(audit) = &session.audit {
                let entries = audit.read_last(last).await?;
                if entries.is_empty() {
                    println!("No audit entries found.");
                } else {
                    for entry in entries {
                        println!("{}", serde_json::to_string(&entry)?);
                    }
                }
            } else {
                println!("Audit logging is disabled.");
            }
        }
        Commands::WorkspaceAdd { path } => {
            let canonical = std::path::Path::new(&path).canonicalize()?;
            println!("Note: config editing not yet implemented.");
            println!("To add {:?}, edit ~/.sc-agent/config.toml:", canonical);
            println!("  [workspace]");
            println!("  allow = [\"{}\"]", canonical.display());
        }
        Commands::Doctor => {
            println!("SC Node Health Check");
            println!("====================");
            println!("\nConfig: OK");
            println!("  Data dir: {}", session.config.data_dir().display());
            println!("  Audit path: {}", session.config.audit_path().display());
            println!("\nProviders ({}):", session.providers.len());
            for p in &session.providers {
                print!("  {} ({}): ", p.name(), p.key());
                match p.health_check().await {
                    Ok(true) => println!("HEALTHY"),
                    Ok(false) => println!("UNHEALTHY"),
                    Err(e) => println!("ERROR: {e}"),
                }
            }
            println!("\nTools ({}):", session.tools.names().len());
            for name in session.tools.names() {
                println!("  - {}", name);
            }
            println!("\nWorkspace ({}):", session.config.workspace.allow.len());
            for p in &session.config.workspace.allow {
                println!("  - {}", p);
            }
            if let Some(audit) = &session.audit {
                match audit.read_last(1).await {
                    Ok(entries) => {
                        println!("\nAudit: ENABLED ({} recent entries)", entries.len());
                    }
                    Err(e) => println!("\nAudit: ENABLED (read error: {e})"),
                }
            } else {
                println!("\nAudit: DISABLED");
            }
        }
        // Already handled above
        Commands::ConfigInit => {}
        Commands::Contract { .. } => {}
        Commands::Proof { .. } => {}
    }

    Ok(())
}

/// Handle `sc-agent contract <validate|explain> <path>`. No config required.
fn run_contract(action: &ContractCmd) -> anyhow::Result<()> {
    match action {
        ContractCmd::Validate { path } => {
            let text = std::fs::read_to_string(path)?;
            let contract = sc_contract::ExecutionContract::parse(&text)?;
            contract.validate()?;
            println!("Contract OK: {}", path);
            println!("Policy hash: {}", contract.policy_hash()?);
            Ok(())
        }
        ContractCmd::Explain { path } => {
            let text = std::fs::read_to_string(path)?;
            let contract = sc_contract::ExecutionContract::parse(&text)?;
            println!("{}", contract.explain());
            Ok(())
        }
    }
}

/// Handle `sc-agent proof verify <path>`. No config required.
fn run_proof(action: &ProofCmd) -> anyhow::Result<()> {
    match action {
        ProofCmd::Verify { path } => {
            let text = std::fs::read_to_string(path)?;
            let bundle: sc_proof::ProofBundle = serde_json::from_str(&text)?;
            sc_proof::verify(&bundle)?;
            sc_proof::check_event_count(&bundle)?;
            println!("Proof OK: {} audit chain verified", path);
            if let Some(head) = sc_proof::chain_head(&bundle.audit_chain) {
                println!("Chain head: {head}");
            }
            Ok(())
        }
    }
}
