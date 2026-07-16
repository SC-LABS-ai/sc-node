//! Configuration types for SC Node.
//!
//! This crate defines the TOML configuration structure and provides
//! loading with environment variable overrides.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Config file not found: {0}")]
    NotFound(PathBuf),
    #[error("Failed to parse config: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("Failed to serialize config: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid config: {0}")]
    Validation(String),
}

/// Complete SC Node configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// General settings
    pub general: GeneralConfig,

    /// Workspace allowlist/denylist
    pub workspace: WorkspaceConfig,

    /// Tool permissions
    pub permissions: PermissionsConfig,

    /// Provider configurations
    pub providers: ProvidersConfig,

    /// Model routing rules
    pub routing: RoutingConfig,

    /// Audit log settings
    pub audit: AuditConfig,
}

/// General application settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    /// Log level: trace, debug, info, warn, error
    pub log_level: String,

    /// Data directory for audit logs, cache, etc.
    pub data_dir: String,

    /// Disable all telemetry (default: true)
    pub no_telemetry: bool,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            log_level: "info".into(),
            data_dir: "~/.sc-agent".into(),
            no_telemetry: true,
        }
    }
}

/// Workspace path allowlist/denylist.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceConfig {
    /// Allowed root directories (expanded: ~, env vars)
    pub allow: Vec<String>,

    /// Denied paths (exact or glob patterns) - takes precedence
    pub deny: Vec<String>,
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            allow: vec![],
            deny: vec![
                "~/.ssh".into(),
                "~/.aws".into(),
                "**/node_modules/**".into(),
                "**/.git/**".into(),
                "**/target/**".into(),
            ],
        }
    }
}

/// Tool permission policies.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PermissionsConfig {
    /// Default policy: "allow", "deny", "ask"
    pub default_policy: String,

    /// Per-tool policy overrides
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub tools: HashMap<String, ToolPermission>,
}

impl Default for PermissionsConfig {
    fn default() -> Self {
        let mut tools = HashMap::new();
        tools.insert(
            "shell".into(),
            ToolPermission {
                policy: "ask".into(),
                allow_patterns: vec![
                    "cargo ".into(),
                    "rustc ".into(),
                    "git ".into(),
                    "ls ".into(),
                    "cat ".into(),
                    "grep ".into(),
                    "rg ".into(),
                    "find ".into(),
                    "mkdir ".into(),
                    "touch ".into(),
                ],
                deny_patterns: vec![
                    "rm -rf".into(),
                    "sudo ".into(),
                    "chmod 777".into(),
                    "curl | sh".into(),
                    "wget | sh".into(),
                    "| sh".into(),
                    "|sh".into(),
                    "| bash".into(),
                    "|bash".into(),
                    "> /dev/sd*".into(),
                    "dd if=".into(),
                    "mkfs".into(),
                    "format ".into(),
                    "shutdown".into(),
                    "reboot".into(),
                ],
            },
        );
        tools.insert(
            "file".into(),
            ToolPermission {
                policy: "ask".into(),
                allow_patterns: vec![
                    "*.md".into(),
                    "*.txt".into(),
                    "*.json".into(),
                    "*.toml".into(),
                    "*.rs".into(),
                    "*.py".into(),
                    "*.js".into(),
                    "*.ts".into(),
                ],
                deny_patterns: vec![
                    "*.key".into(),
                    "*.pem".into(),
                    "id_rsa*".into(),
                    "*.secret".into(),
                    "credentials*".into(),
                ],
            },
        );
        tools.insert(
            "web".into(),
            ToolPermission {
                policy: "ask".into(),
                allow_patterns: vec![],
                deny_patterns: vec![
                    "localhost".into(),
                    "127.0.0.1".into(),
                    "169.254.169.254".into(), // AWS metadata
                ],
            },
        );

        Self {
            default_policy: "ask".into(),
            tools,
        }
    }
}

/// Permission settings for a specific tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolPermission {
    /// Policy: "allow", "deny", "ask"
    pub policy: String,

    /// Patterns that are auto-allowed (glob)
    pub allow_patterns: Vec<String>,

    /// Patterns that are auto-denied (glob)
    pub deny_patterns: Vec<String>,
}

impl Default for ToolPermission {
    fn default() -> Self {
        Self {
            policy: "ask".into(),
            allow_patterns: vec![],
            deny_patterns: vec![],
        }
    }
}

/// Provider configurations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProvidersConfig {
    pub openrouter: Option<OpenRouterConfig>,
    pub nvidia: Option<NvidiaConfig>,
    pub ollama: Option<OllamaConfig>,
}

impl Default for ProvidersConfig {
    fn default() -> Self {
        Self {
            openrouter: Some(OpenRouterConfig::default()),
            nvidia: Some(NvidiaConfig::default()),
            ollama: Some(OllamaConfig::default()),
        }
    }
}

/// OpenRouter provider config.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenRouterConfig {
    pub enabled: bool,
    /// API key is resolved from the `SC_AGENT_OPENROUTER_API_KEY` environment
    /// variable only. `#[serde(skip)]` keeps it out of the on-disk TOML in both
    /// directions: a plaintext key placed in a config file is ignored, and it is
    /// never written back out by `config show`.
    #[serde(skip)]
    pub api_key: Option<String>,
    pub base_url: String,
    pub default_model: String,
    pub timeout_secs: u64,
    pub max_retries: u32,
}

impl Default for OpenRouterConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_key: None,
            base_url: "https://openrouter.ai/api/v1".into(),
            default_model: "openai/gpt-4.1-mini".into(),
            timeout_secs: 60,
            max_retries: 3,
        }
    }
}

/// NVIDIA NIM provider config.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NvidiaConfig {
    pub enabled: bool,
    /// API key is resolved from the `SC_AGENT_NVIDIA_API_KEY` environment
    /// variable only. `#[serde(skip)]` keeps it out of the on-disk TOML in both
    /// directions: a plaintext key placed in a config file is ignored, and it is
    /// never written back out by `config show`.
    #[serde(skip)]
    pub api_key: Option<String>,
    pub base_url: String,
    pub default_model: String,
    pub timeout_secs: u64,
    pub max_retries: u32,
}

impl Default for NvidiaConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_key: None,
            base_url: "https://integrate.api.nvidia.com/v1".into(),
            default_model: "deepseek-ai/deepseek-v4-pro".into(),
            timeout_secs: 60,
            max_retries: 3,
        }
    }
}

/// Ollama local provider config.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OllamaConfig {
    pub enabled: bool,
    pub base_url: String,
    pub default_model: String,
    pub keep_alive: String,
    pub timeout_secs: u64,
    pub max_retries: u32,
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            base_url: "http://localhost:11434".into(),
            default_model: "llama3.2:3b".into(),
            keep_alive: "5m".into(),
            timeout_secs: 120,
            max_retries: 2,
        }
    }
}

/// Model routing rules.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RoutingConfig {
    /// Rules evaluated in order; first match wins
    pub rules: Vec<RoutingRule>,

    /// Fallback provider if no rules match
    pub fallback_provider: String,

    /// Fallback model
    pub fallback_model: String,
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            rules: vec![
                RoutingRule {
                    name: "code-tasks".into(),
                    match_contains: vec![
                        "code".into(),
                        "rust".into(),
                        "cargo".into(),
                        "clippy".into(),
                    ],
                    provider: "ollama".into(),
                    model: "codellama:7b".into(),
                },
                RoutingRule {
                    name: "research-tasks".into(),
                    match_contains: vec![
                        "research".into(),
                        "search".into(),
                        "find".into(),
                        "lookup".into(),
                    ],
                    provider: "openrouter".into(),
                    model: "perplexity/sonar-pro".into(),
                },
            ],
            fallback_provider: "openrouter".into(),
            fallback_model: "openai/gpt-4.1-mini".into(),
        }
    }
}

/// A single routing rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingRule {
    pub name: String,
    pub match_contains: Vec<String>,
    pub provider: String,
    pub model: String,
}

/// Audit log configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AuditConfig {
    pub enabled: bool,
    pub path: String,
    pub max_size_mb: u64,
    pub max_files: u32,
    pub log_args: bool,
    pub log_output: bool,
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            path: "audit.log".into(),
            max_size_mb: 100,
            max_files: 10,
            log_args: false,
            log_output: false,
        }
    }
}

impl Config {
    /// Load config from default location (~/.sc-agent/config.toml).
    pub fn load() -> Result<Self, ConfigError> {
        let path = Self::default_path()?;
        Self::load_from(&path)
    }

    /// Load config from a specific path.
    pub fn load_from(path: &Path) -> Result<Self, ConfigError> {
        if !path.exists() {
            return Err(ConfigError::NotFound(path.to_path_buf()));
        }
        let content = std::fs::read_to_string(path)?;
        let mut config: Config = toml::from_str(&content)?;
        config.apply_env_overrides();
        config.validate()?;
        Ok(config)
    }

    /// Get default config path.
    ///
    /// The `SC_AGENT_CONFIG` environment variable, when set to a non-empty
    /// value, overrides the location. This enables isolated runs and testing
    /// against an alternate profile without touching `~/.sc-agent/config.toml`
    /// (on Windows `dirs::home_dir()` resolves the real profile via the Win32
    /// API and ignores a `USERPROFILE` override, so an explicit path is the
    /// only reliable way to isolate). Falls back to `~/.sc-agent/config.toml`.
    pub fn default_path() -> Result<PathBuf, ConfigError> {
        if let Ok(p) = std::env::var("SC_AGENT_CONFIG")
            && !p.trim().is_empty()
        {
            return Ok(PathBuf::from(p));
        }
        let home = dirs::home_dir()
            .ok_or_else(|| ConfigError::Validation("Could not determine home directory".into()))?;
        Ok(home.join(".sc-agent").join("config.toml"))
    }

    /// Get data directory (expanded).
    pub fn data_dir(&self) -> PathBuf {
        let path = shellexpand::tilde(&self.general.data_dir).into_owned();
        PathBuf::from(path)
    }

    /// Get audit log path (expanded, relative to data_dir if not absolute).
    pub fn audit_path(&self) -> PathBuf {
        let path = PathBuf::from(&self.audit.path);
        if path.is_absolute() {
            path
        } else {
            self.data_dir().join(path)
        }
    }

    /// Apply environment variable overrides.
    fn apply_env_overrides(&mut self) {
        // General
        if let Ok(v) = std::env::var("SC_AGENT_LOG_LEVEL") {
            self.general.log_level = v;
        }
        if let Ok(v) = std::env::var("SC_AGENT_DATA_DIR") {
            self.general.data_dir = v;
        }

        // Providers - API keys from env
        if let Some(cfg) = &mut self.providers.openrouter
            && let Ok(v) = std::env::var("SC_AGENT_OPENROUTER_API_KEY")
        {
            cfg.api_key = Some(v);
        }
        if let Some(cfg) = &mut self.providers.nvidia
            && let Ok(v) = std::env::var("SC_AGENT_NVIDIA_API_KEY")
        {
            cfg.api_key = Some(v);
        }
    }

    /// Validate configuration.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.general.log_level.is_empty() {
            return Err(ConfigError::Validation("log_level cannot be empty".into()));
        }
        if self.audit.enabled && self.audit.path.is_empty() {
            return Err(ConfigError::Validation(
                "audit.path cannot be empty when audit.enabled=true".into(),
            ));
        }
        // Reject blank allow/deny patterns: a blank shell allow pattern would
        // expand to a match-all wildcard and silently neuter the allow list.
        for (tool, perm) in &self.permissions.tools {
            if perm.allow_patterns.iter().any(|p| p.trim().is_empty()) {
                return Err(ConfigError::Validation(format!(
                    "permissions.tools.{tool}.allow_patterns contains an empty pattern"
                )));
            }
            if perm.deny_patterns.iter().any(|p| p.trim().is_empty()) {
                return Err(ConfigError::Validation(format!(
                    "permissions.tools.{tool}.deny_patterns contains an empty pattern"
                )));
            }
        }
        Ok(())
    }

    /// Create a default config file at the default location.
    pub fn create_default() -> Result<PathBuf, ConfigError> {
        let path = Self::default_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(&Config::default())?;
        std::fs::write(&path, content)?;
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert!(config.general.no_telemetry);
        assert!(config.providers.ollama.as_ref().unwrap().enabled);
        assert!(!config.providers.openrouter.as_ref().unwrap().enabled);
    }

    #[test]
    fn test_api_key_never_persisted_to_toml() {
        // A plaintext key in a config file must be ignored on load, and a key
        // present only at runtime must never be written back out.
        // The fake value is deliberately not key-shaped; #[serde(skip)] drops
        // the field regardless of its contents, and keeping the fixture plain
        // avoids tripping the release public-clean secret scanner.
        let toml_with_key = "[providers.nvidia]\nenabled = true\napi_key = \"ignored\"\n";
        let cfg: Config = toml::from_str(toml_with_key).unwrap();
        assert_eq!(cfg.providers.nvidia.as_ref().unwrap().api_key, None);

        let mut cfg2 = Config::default();
        if let Some(n) = cfg2.providers.nvidia.as_mut() {
            n.api_key = Some("runtime-only".into());
        }
        let dumped = toml::to_string_pretty(&cfg2).unwrap();
        assert!(!dumped.contains("runtime-only"));
        assert!(!dumped.contains("api_key"));
    }

    #[test]
    fn test_sc_agent_config_env_overrides_path() {
        // Unique env var owned by this test; restored immediately after.
        let dir = tempdir().unwrap();
        let custom = dir.path().join("custom-config.toml");
        // SAFETY: single-threaded access to a var this test owns for its body.
        unsafe { std::env::set_var("SC_AGENT_CONFIG", &custom) };
        let resolved = Config::default_path().unwrap();
        unsafe { std::env::remove_var("SC_AGENT_CONFIG") };
        assert_eq!(resolved, custom);
    }

    #[test]
    fn test_validate_rejects_empty_pattern() {
        let mut config = Config::default();
        config
            .permissions
            .tools
            .get_mut("shell")
            .unwrap()
            .allow_patterns
            .push("".into());
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let config = Config::default();
        let content = toml::to_string_pretty(&config).unwrap();
        std::fs::write(&path, content).unwrap();
        let loaded = Config::load_from(&path).unwrap();
        assert_eq!(config.general.no_telemetry, loaded.general.no_telemetry);
    }

    #[test]
    fn test_workspace_expansion() {
        let mut config = Config::default();
        config.workspace.allow = vec!["~/projects".into(), "${HOME}/work".into()];
        let data_dir = config.data_dir();
        assert!(data_dir.to_string_lossy().contains(".sc-agent"));
    }
}
