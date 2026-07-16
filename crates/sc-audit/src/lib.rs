//! Append-only audit logging for SC Node.
//!
//! This crate provides tamper-evident audit logging for all tool executions.

use std::fs::OpenOptions;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use chrono::Utc;
use sc_config::AuditConfig;
use sc_message_types::{AuditDecision, AuditEntry, SessionId};
use tokio::sync::Mutex;

/// Audit logger with append-only file writing.
pub struct AuditLogger {
    file: Arc<Mutex<Option<std::fs::File>>>,
    config: AuditConfig,
    current_size: Arc<Mutex<u64>>,
}

impl AuditLogger {
    /// Create a new audit logger.
    pub async fn new(config: AuditConfig) -> Result<Self> {
        let path = Path::new(&config.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let file = OpenOptions::new().create(true).append(true).open(path)?;

        let metadata = file.metadata()?;
        let current_size = metadata.len();

        Ok(Self {
            file: Arc::new(Mutex::new(Some(file))),
            config,
            current_size: Arc::new(Mutex::new(current_size)),
        })
    }

    /// Log an audit entry.
    pub async fn log(&self, entry: AuditEntry) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }

        let line = serde_json::to_string(&entry)? + "\n";

        // Check rotation
        let mut size = self.current_size.lock().await;
        let new_size = *size + line.len() as u64;

        if new_size > self.config.max_size_mb * 1024 * 1024 {
            self.rotate().await?;
            *size = 0;
        } else {
            *size = new_size;
        }

        // Write entry
        let mut file_guard = self.file.lock().await;
        if let Some(file) = file_guard.as_mut() {
            use std::io::Write;
            file.write_all(line.as_bytes())?;
            file.flush()?;
        }

        Ok(())
    }

    /// Rotate the audit log.
    async fn rotate(&self) -> Result<()> {
        let path = Path::new(&self.config.path);

        // Close current file
        {
            let mut file_guard = self.file.lock().await;
            *file_guard = None;
        }

        // Rotate existing files
        for i in (1..self.config.max_files).rev() {
            let src = path.with_extension(format!("log.{}", i));
            let dst = path.with_extension(format!("log.{}", i + 1));
            if src.exists() {
                if i + 1 >= self.config.max_files {
                    std::fs::remove_file(&src)?;
                } else {
                    std::fs::rename(&src, &dst)?;
                }
            }
        }

        // Move current to .1
        let first_rotated = path.with_extension("log.1");
        std::fs::rename(path, &first_rotated)?;

        // Open new file
        let file = OpenOptions::new().create(true).append(true).open(path)?;

        *self.file.lock().await = Some(file);

        Ok(())
    }

    /// Read the last N entries from the audit log (most recent first).
    pub async fn read_last(&self, n: usize) -> Result<Vec<AuditEntry>> {
        let path = Path::new(&self.config.path);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let content = std::fs::read_to_string(path)?;
        let mut entries: Vec<AuditEntry> = content
            .lines()
            .rev()
            .take(n)
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        entries.reverse();
        Ok(entries)
    }

    /// Create a standard audit entry for a tool call.
    #[allow(clippy::too_many_arguments)]
    pub fn create_entry(
        session_id: SessionId,
        tool: impl Into<String>,
        _args: Option<serde_json::Value>,
        policy: impl Into<String>,
        decision: AuditDecision,
        exit_code: Option<i32>,
        duration_ms: u64,
        error: Option<String>,
    ) -> AuditEntry {
        AuditEntry {
            timestamp: Utc::now(),
            session_id,
            tool: tool.into(),
            args: None,
            policy: policy.into(),
            decision,
            exit_code,
            duration_ms,
            error,
            output: None,
        }
    }
}

/// Convenience function to create an audit entry.
#[allow(clippy::too_many_arguments)]
pub fn create_audit_entry(
    session_id: SessionId,
    tool: impl Into<String>,
    _args: Option<serde_json::Value>,
    policy: impl Into<String>,
    decision: AuditDecision,
    exit_code: Option<i32>,
    duration_ms: u64,
    error: Option<String>,
    log_args: bool,
    log_output: bool,
    output: Option<String>,
) -> AuditEntry {
    AuditEntry {
        timestamp: Utc::now(),
        session_id,
        tool: tool.into(),
        args: if log_args { _args } else { None },
        policy: policy.into(),
        decision,
        exit_code,
        duration_ms,
        error,
        output: if log_output { output } else { None },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_message_types::SessionId;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_audit_log_basic() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("audit.log");

        let config = AuditConfig {
            enabled: true,
            path: path.to_string_lossy().to_string(),
            max_size_mb: 1,
            max_files: 3,
            log_args: true,
            log_output: false,
        };

        let logger = AuditLogger::new(config).await.unwrap();

        let entry = create_audit_entry(
            SessionId::new(),
            "shell",
            Some(serde_json::json!({"cmd": ["echo", "hello"]})),
            "allow_pattern",
            AuditDecision::Allowed,
            Some(0),
            10,
            None,
            true,
            false,
            None,
        );

        logger.log(entry).await.unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("shell"));
        assert!(content.contains("echo"));
    }

    #[tokio::test]
    async fn test_audit_log_tool_success() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("audit.log");

        let config = AuditConfig {
            enabled: true,
            path: path.to_string_lossy().to_string(),
            max_size_mb: 1,
            max_files: 3,
            log_args: true,
            log_output: true,
        };

        let logger = AuditLogger::new(config).await.unwrap();

        let entry = create_audit_entry(
            SessionId::new(),
            "shell",
            Some(serde_json::json!({"cmd": ["echo", "hello"]})),
            "allow_pattern",
            AuditDecision::Allowed,
            Some(0),
            10,
            None,
            true,
            true,
            Some("hello\n".to_string()),
        );

        logger.log(entry).await.unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("shell"));
        assert!(content.contains("echo"));
        assert!(content.contains("hello"));
    }

    #[tokio::test]
    async fn test_audit_log_tool_error() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("audit.log");

        let config = AuditConfig {
            enabled: true,
            path: path.to_string_lossy().to_string(),
            max_size_mb: 1,
            max_files: 3,
            log_args: true,
            log_output: false,
        };

        let logger = AuditLogger::new(config).await.unwrap();

        let entry = create_audit_entry(
            SessionId::new(),
            "shell",
            Some(serde_json::json!({"cmd": ["false"]})),
            "allow_pattern",
            AuditDecision::Error,
            Some(1),
            50,
            Some("command failed".to_string()),
            true,
            false,
            None,
        );

        logger.log(entry).await.unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("shell"));
        assert!(content.contains("command failed"));
    }

    #[tokio::test]
    async fn test_audit_log_unknown_tool() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("audit.log");

        let config = AuditConfig {
            enabled: true,
            path: path.to_string_lossy().to_string(),
            max_size_mb: 1,
            max_files: 3,
            log_args: true,
            log_output: false,
        };

        let logger = AuditLogger::new(config).await.unwrap();

        let entry = create_audit_entry(
            SessionId::new(),
            "nonexistent",
            Some(serde_json::json!({})),
            "allow_pattern",
            AuditDecision::Denied,
            Some(127),
            1,
            Some("Tool 'nonexistent' not found".to_string()),
            true,
            false,
            None,
        );

        logger.log(entry).await.unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("nonexistent"));
        assert!(content.contains("not found"));
    }

    #[tokio::test]
    async fn test_audit_disabled_writes_no_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("audit.log");

        let config = AuditConfig {
            enabled: false,
            path: path.to_string_lossy().to_string(),
            max_size_mb: 1,
            max_files: 3,
            log_args: true,
            log_output: false,
        };

        let logger = AuditLogger::new(config).await.unwrap();

        let entry = create_audit_entry(
            SessionId::new(),
            "shell",
            Some(serde_json::json!({"cmd": ["echo", "hello"]})),
            "allow_pattern",
            AuditDecision::Allowed,
            Some(0),
            10,
            None,
            true,
            false,
            None,
        );

        logger.log(entry).await.unwrap();

        // When disabled, no entry should be written to the file
        // The file may exist (created on logger creation) but should be empty
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.is_empty());
    }

    #[tokio::test]
    async fn test_audit_log_args_false_hides_args() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("audit.log");

        let config = AuditConfig {
            enabled: true,
            path: path.to_string_lossy().to_string(),
            max_size_mb: 1,
            max_files: 3,
            log_args: false,
            log_output: false,
        };

        let logger = AuditLogger::new(config).await.unwrap();

        let entry = create_audit_entry(
            SessionId::new(),
            "shell",
            Some(serde_json::json!({"cmd": ["echo", "secret"]})),
            "allow_pattern",
            AuditDecision::Allowed,
            Some(0),
            10,
            None,
            false,
            false,
            None,
        );

        logger.log(entry).await.unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("shell"));
        assert!(!content.contains("secret"));
        // When log_args=false, args should be null in JSON
        assert!(content.contains("\"args\":null"));
    }

    #[tokio::test]
    async fn test_audit_log_output_false_hides_output() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("audit.log");

        let config = AuditConfig {
            enabled: true,
            path: path.to_string_lossy().to_string(),
            max_size_mb: 1,
            max_files: 3,
            log_args: true,
            log_output: false,
        };

        let logger = AuditLogger::new(config).await.unwrap();

        // Test with log_output=false: output field should be null in JSON
        let entry = create_audit_entry(
            SessionId::new(),
            "shell",
            Some(serde_json::json!({"cmd": ["echo", "secret_output"]})),
            "allow_pattern",
            AuditDecision::Allowed,
            Some(0),
            10,
            None,
            true,
            false,
            Some("secret_output\n".to_string()),
        );

        logger.log(entry).await.unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("shell"));
        // secret_output is in args (as part of cmd), not in output field
        // When log_output=false, output field should be null
        assert!(content.contains("\"output\":null"));
    }

    #[tokio::test]
    async fn test_audit_log_output_true_includes_output() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("audit.log");

        let config = AuditConfig {
            enabled: true,
            path: path.to_string_lossy().to_string(),
            max_size_mb: 1,
            max_files: 3,
            log_args: true,
            log_output: true,
        };

        let logger = AuditLogger::new(config).await.unwrap();

        let entry = create_audit_entry(
            SessionId::new(),
            "shell",
            Some(serde_json::json!({"cmd": ["echo", "visible_output"]})),
            "allow_pattern",
            AuditDecision::Allowed,
            Some(0),
            10,
            None,
            true,
            true,
            Some("visible_output\n".to_string()),
        );

        logger.log(entry).await.unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("shell"));
        assert!(content.contains("visible_output"));
    }
}
