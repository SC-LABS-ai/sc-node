//! File tools for SC Node (read, write, list).

use async_trait::async_trait;
use sc_message_types::ToolResult;
use sc_sandbox::resolve_and_check_path;
use sc_tool_core::{Tool, ToolContext, ToolError};
use std::path::Path;

/// Read file tool.
pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }
    fn description(&self) -> &str {
        "Read the contents of a file"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to the file to read" }
            },
            "required": ["path"]
        })
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        // Belt-and-suspenders: the central dispatch gate in sc-agent-core is
        // authoritative for both Deny and Ask (it fails closed on Ask when
        // non-interactive), so this call is denied before we ever get here.
        // This internal check only catches Deny for callers that invoke the
        // tool directly, bypassing the gate.
        if let sc_tool_core::PermissionDecision::Deny(reason) =
            sc_tool_core::check_permission(self.name(), &input, &context.permissions)
        {
            return Err(ToolError::PermissionDenied(reason));
        }

        let path = input
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("Missing 'path' parameter".into()))?;

        let safe_path = resolve_and_check_path(
            Path::new(path),
            &context.working_dir,
            &context.permissions.workspace_config,
        )
        .map_err(|e| ToolError::PathNotAllowed(e.to_string()))?;

        let content = tokio::fs::read_to_string(&safe_path)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        Ok(ToolResult {
            tool_call_id: "".into(), // Set by caller
            output: content,
            is_error: false,
            exit_code: Some(0),
        })
    }
}

/// Write file tool.
pub struct WriteFileTool;

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }
    fn description(&self) -> &str {
        "Write content to a file"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to the file to write" },
                "content": { "type": "string", "description": "Content to write" }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        // Belt-and-suspenders: the central dispatch gate in sc-agent-core is
        // authoritative for both Deny and Ask (it fails closed on Ask when
        // non-interactive), so this call is denied before we ever get here.
        // This internal check only catches Deny for callers that invoke the
        // tool directly, bypassing the gate.
        if let sc_tool_core::PermissionDecision::Deny(reason) =
            sc_tool_core::check_permission(self.name(), &input, &context.permissions)
        {
            return Err(ToolError::PermissionDenied(reason));
        }

        let path = input
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("Missing 'path' parameter".into()))?;
        let content = input
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("Missing 'content' parameter".into()))?;

        let safe_path = resolve_and_check_path(
            Path::new(path),
            &context.working_dir,
            &context.permissions.workspace_config,
        )
        .map_err(|e| ToolError::PathNotAllowed(e.to_string()))?;

        // Ensure parent directory exists
        if let Some(parent) = safe_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        }

        tokio::fs::write(&safe_path, content)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        Ok(ToolResult {
            tool_call_id: "".into(),
            output: format!("Written to {}", safe_path.display()),
            is_error: false,
            exit_code: Some(0),
        })
    }
}

/// List directory tool.
pub struct ListDirTool;

#[async_trait]
impl Tool for ListDirTool {
    fn name(&self) -> &str {
        "list_dir"
    }
    fn description(&self) -> &str {
        "List files in a directory"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Directory path to list" }
            },
            "required": ["path"]
        })
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        // Belt-and-suspenders: the central dispatch gate in sc-agent-core is
        // authoritative for both Deny and Ask (it fails closed on Ask when
        // non-interactive), so this call is denied before we ever get here.
        // This internal check only catches Deny for callers that invoke the
        // tool directly, bypassing the gate.
        if let sc_tool_core::PermissionDecision::Deny(reason) =
            sc_tool_core::check_permission(self.name(), &input, &context.permissions)
        {
            return Err(ToolError::PermissionDenied(reason));
        }

        let path = input
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("Missing 'path' parameter".into()))?;

        let safe_path = resolve_and_check_path(
            Path::new(path),
            &context.working_dir,
            &context.permissions.workspace_config,
        )
        .map_err(|e| ToolError::PathNotAllowed(e.to_string()))?;

        let mut entries = tokio::fs::read_dir(&safe_path)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        let mut result = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?
        {
            let path = entry.path();
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            let is_dir = path.is_dir();
            result.push(serde_json::json!({
                "name": name,
                "is_dir": is_dir,
                "path": path.to_string_lossy()
            }));
        }

        Ok(ToolResult {
            tool_call_id: "".into(),
            output: serde_json::to_string_pretty(&result).unwrap_or_default(),
            is_error: false,
            exit_code: Some(0),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ListDirTool, ReadFileTool, WriteFileTool};
    use sc_config::{PermissionsConfig, ToolPermission, WorkspaceConfig};
    use sc_message_types::SessionId;
    use sc_tool_core::{
        PermissionDecision, Tool, ToolContext, ToolError, ToolPermissions, check_permission,
    };
    use std::collections::HashMap;

    /// Default allow/deny patterns mirroring `sc-config`'s file defaults,
    /// used by the must-deny/must-allow matrix below.
    fn default_file_patterns() -> (Vec<String>, Vec<String>) {
        let allow = vec![
            "*.md".into(),
            "*.txt".into(),
            "*.json".into(),
            "*.toml".into(),
            "*.rs".into(),
            "*.py".into(),
            "*.js".into(),
            "*.ts".into(),
        ];
        let deny = vec![
            "*.key".into(),
            "*.pem".into(),
            "id_rsa*".into(),
            "*.secret".into(),
            "credentials*".into(),
        ];
        (allow, deny)
    }

    fn create_file_permissions(
        allow_patterns: Vec<String>,
        deny_patterns: Vec<String>,
    ) -> ToolPermissions {
        let mut tools = HashMap::new();
        tools.insert(
            "file".into(),
            ToolPermission {
                policy: "ask".into(),
                allow_patterns,
                deny_patterns,
            },
        );
        let config = PermissionsConfig {
            default_policy: "deny".into(),
            tools,
        };
        ToolPermissions::from_config(&config, WorkspaceConfig::default())
    }

    fn assert_deny(path: &str) {
        let (allow, deny) = default_file_patterns();
        let permissions = create_file_permissions(allow, deny);
        let args = serde_json::json!({"path": path});
        let decision = check_permission("file", &args, &permissions);
        assert!(
            matches!(decision, PermissionDecision::Deny(_)),
            "expected Deny for '{path}', got {decision:?}"
        );
    }

    fn assert_ask(path: &str) {
        let (allow, deny) = default_file_patterns();
        let permissions = create_file_permissions(allow, deny);
        let args = serde_json::json!({"path": path});
        let decision = check_permission("file", &args, &permissions);
        assert!(
            matches!(decision, PermissionDecision::Ask(_)),
            "expected Ask for '{path}', got {decision:?}"
        );
    }

    // --- FILE MUST-DENY --------------------------------------------------

    #[test]
    fn test_file_must_deny_sensitive_names() {
        assert_deny("secret.key");
        assert_deny("private.pem");
        assert_deny("id_rsa");
        assert_deny("/home/user/.ssh/id_rsa");
        assert_deny("credentials.secret");
        assert_deny("SECRET.KEY");
    }

    #[test]
    fn test_file_deny_wins_over_allow_on_overlap() {
        // "id_rsa.md" matches allow "*.md" AND deny "id_rsa*"; deny must win.
        assert_deny("id_rsa.md");
    }

    // --- RUNTIME ENFORCEMENT (proves check_permission is wired into the tools)

    fn ctx_with_file_perms() -> ToolContext {
        let (allow, deny) = default_file_patterns();
        ToolContext {
            session_id: SessionId::new(),
            working_dir: std::env::temp_dir(),
            permissions: create_file_permissions(allow, deny),
        }
    }

    #[tokio::test]
    async fn test_read_file_execute_denies_secret_pattern() {
        // Deny short-circuits before any filesystem access, so no real file
        // is touched. This is the test that catches an unwired file tool.
        let err = ReadFileTool
            .execute(
                serde_json::json!({"path": "secret.key"}),
                ctx_with_file_perms(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::PermissionDenied(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn test_write_file_execute_denies_id_rsa() {
        let err = WriteFileTool
            .execute(
                serde_json::json!({"path": "id_rsa", "content": "x"}),
                ctx_with_file_perms(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::PermissionDenied(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn test_list_dir_execute_denies_credentials() {
        let err = ListDirTool
            .execute(
                serde_json::json!({"path": "credentials.secret"}),
                ctx_with_file_perms(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::PermissionDenied(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn test_read_file_execute_allows_pattern_passes_permission_layer() {
        // An allowed pattern must NOT be blocked by the permission layer. It
        // proceeds to the workspace path check, which denies here only because
        // the default test workspace allow-list is empty. Proves enforcement is
        // pattern-aware, not a blanket deny.
        let err = ReadFileTool
            .execute(
                serde_json::json!({"path": "notes.md"}),
                ctx_with_file_perms(),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, ToolError::PathNotAllowed(_)),
            "expected PathNotAllowed (permission layer passed), got {err:?}"
        );
    }

    #[test]
    fn test_file_must_deny_anchored_allow_does_not_match_disguised_extension() {
        // "*.md" is an anchored whole-string match against the filename or
        // full path; it must not match a filename that merely contains
        // ".md" followed by more characters.
        assert_deny("notes.md.exe");
    }

    #[test]
    fn test_file_path_missing_is_denied() {
        let (allow, deny) = default_file_patterns();
        let permissions = create_file_permissions(allow, deny);
        let args = serde_json::json!({});
        let decision = check_permission("file", &args, &permissions);
        assert!(matches!(decision, PermissionDecision::Deny(_)));
    }

    // --- FILE MUST-ALLOW -> Ask -------------------------------------------

    #[test]
    fn test_file_must_allow_to_ask() {
        assert_ask("README.md");
        assert_ask("dir/sub/notes.txt");
        assert_ask("dir\\sub\\notes.txt");
    }

    // --- Legacy scenarios kept for regression coverage, corrected --------

    #[test]
    fn test_file_deny_pattern_sensitive() {
        let permissions = create_file_permissions(
            vec!["*.md".into(), "*.txt".into()],
            vec![
                "*.key".into(),
                "*.pem".into(),
                "id_rsa*".into(),
                "*.secret".into(),
            ],
        );
        let args = serde_json::json!({"path": "/home/user/.ssh/id_rsa"});
        let decision = check_permission("file", &args, &permissions);
        // Deny pattern "id_rsa*" matches the filename "id_rsa" -> Deny.
        assert!(matches!(decision, PermissionDecision::Deny(_)));
    }

    #[test]
    fn test_file_allow_pattern_md() {
        let permissions =
            create_file_permissions(vec!["*.md".into(), "*.txt".into()], vec!["*.key".into()]);
        let args = serde_json::json!({"path": "README.md"});
        let decision = check_permission("file", &args, &permissions);
        assert!(matches!(decision, PermissionDecision::Ask(_)));
    }

    #[test]
    fn test_file_no_patterns_ask_policy_maps_to_ask() {
        let permissions = create_file_permissions(vec![], vec![]);
        let args = serde_json::json!({"path": "unknown.exe"});
        let decision = check_permission("file", &args, &permissions);
        assert!(matches!(decision, PermissionDecision::Ask(_)));
    }

    // --- Phase 3: hardened sandbox boundary, exercised end-to-end through
    // the real tools (not just sc-sandbox's own unit tests). Permission
    // policy is "allow" here so these tests isolate the workspace-boundary
    // layer (resolve_and_check_path), not the pattern-permission layer
    // already covered above.

    fn ctx_with_workspace(root: &std::path::Path) -> ToolContext {
        let mut tools = HashMap::new();
        tools.insert(
            "file".into(),
            ToolPermission {
                policy: "allow".into(),
                allow_patterns: vec![],
                deny_patterns: vec![],
            },
        );
        let permissions_config = PermissionsConfig {
            default_policy: "allow".into(),
            tools,
        };
        let workspace_config = WorkspaceConfig {
            allow: vec![root.to_string_lossy().to_string()],
            deny: vec![],
        };
        ToolContext {
            session_id: SessionId::new(),
            working_dir: root.to_path_buf(),
            permissions: ToolPermissions::from_config(&permissions_config, workspace_config),
        }
    }

    #[tokio::test]
    async fn test_read_file_denies_reserved_device_name_inside_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let err = ReadFileTool
            .execute(
                serde_json::json!({"path": "CON"}),
                ctx_with_workspace(dir.path()),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::PathNotAllowed(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn test_read_file_denies_alternate_data_stream_reference() {
        let dir = tempfile::tempdir().unwrap();
        let err = ReadFileTool
            .execute(
                serde_json::json!({"path": "secret.key:$DATA"}),
                ctx_with_workspace(dir.path()),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::PathNotAllowed(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn test_write_then_read_file_case_insensitive_path_same_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_with_workspace(dir.path());

        let write_result = WriteFileTool
            .execute(
                serde_json::json!({"path": "Notes.txt", "content": "hello"}),
                ctx_with_workspace(dir.path()),
            )
            .await
            .unwrap();
        assert!(!write_result.is_error);

        // Re-request the same file via a differently-cased path; the
        // workspace boundary check must not reject it (Windows filesystems
        // are case-insensitive), even though the requested case differs
        // from what was written.
        let upper_path = dir
            .path()
            .to_string_lossy()
            .to_ascii_uppercase()
            .replace(['/', '\\'], "\\");
        let read_result = ReadFileTool
            .execute(
                serde_json::json!({"path": format!("{upper_path}\\NOTES.TXT")}),
                ctx,
            )
            .await
            .unwrap();
        assert!(!read_result.is_error);
        assert_eq!(read_result.output, "hello");
    }

    #[tokio::test]
    async fn test_write_file_denies_workspace_prefix_confusion() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("allowed");
        std::fs::create_dir_all(&workspace).unwrap();

        let mut tools = HashMap::new();
        tools.insert(
            "file".into(),
            ToolPermission {
                policy: "allow".into(),
                allow_patterns: vec![],
                deny_patterns: vec![],
            },
        );
        let permissions_config = PermissionsConfig {
            default_policy: "allow".into(),
            tools,
        };
        let workspace_config = WorkspaceConfig {
            allow: vec![workspace.to_string_lossy().to_string()],
            deny: vec![],
        };
        let ctx = ToolContext {
            session_id: SessionId::new(),
            working_dir: dir.path().to_path_buf(),
            permissions: ToolPermissions::from_config(&permissions_config, workspace_config),
        };

        // "allowed-evil" is a sibling of "allowed", not a subdirectory of
        // it; the allow entry for "allowed" must not authorize it.
        let evil_path = dir
            .path()
            .join("allowed-evil")
            .join("file.txt")
            .to_string_lossy()
            .to_string();
        let err = WriteFileTool
            .execute(serde_json::json!({"path": evil_path, "content": "x"}), ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::PathNotAllowed(_)), "got {err:?}");
    }
}
