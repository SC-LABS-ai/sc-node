//! Path sandboxing and process isolation for SC Node.

use sc_config::WorkspaceConfig;
use std::path::{Component, Path, PathBuf, Prefix};
use std::process::Stdio;
use std::time::{Duration, Instant};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SandboxError {
    #[error("Path not allowed: {0}")]
    PathNotAllowed(PathBuf),

    #[error("Path resolution failed: {0}")]
    ResolutionFailed(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub struct SandboxedOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
}

impl SandboxedOutput {
    pub fn success(&self) -> bool {
        self.exit_code == Some(0)
    }

    pub fn combined_output(&self) -> String {
        if self.stderr.trim().is_empty() {
            self.stdout.clone()
        } else if self.stdout.trim().is_empty() {
            self.stderr.clone()
        } else {
            format!("{}\n{}", self.stdout, self.stderr)
        }
    }
}

#[derive(Debug, Clone)]
pub struct SandboxedCommand {
    program: String,
    args: Vec<String>,
    working_dir: Option<PathBuf>,
    timeout: Duration,
}

impl SandboxedCommand {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            working_dir: None,
            timeout: Duration::from_secs(300),
        }
    }

    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn working_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.working_dir = Some(path.into());
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub async fn execute(
        &self,
        workspace: &WorkspaceConfig,
    ) -> Result<SandboxedOutput, SandboxError> {
        let cwd = self
            .working_dir
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

        if !is_path_allowed(&cwd, workspace) {
            return Err(SandboxError::PathNotAllowed(cwd));
        }

        let started = Instant::now();

        let output = tokio::process::Command::new(&self.program)
            .args(&self.args)
            .current_dir(&cwd)
            .stdin(Stdio::null())
            .output()
            .await?;

        Ok(SandboxedOutput {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code(),
            duration_ms: started.elapsed().as_millis() as u64,
        })
    }
}

pub fn check_path_allowed(path: &Path, config: &WorkspaceConfig) -> bool {
    is_path_allowed(path, config)
}

pub fn is_path_allowed(path: &Path, config: &WorkspaceConfig) -> bool {
    let normalized = match normalize_path(path) {
        Ok(p) => p,
        Err(_) => return false,
    };

    if !is_path_allowed_syntactic(&normalized, config) {
        return false;
    }

    // Defense in depth against symlink/junction escapes: `normalize_path`
    // is purely syntactic (it never touches the filesystem, so pre-creation
    // checks like `write_file` on a not-yet-existing path still work). That
    // means a reparse point (symlink or junction) *inside* the workspace
    // that points *outside* it would otherwise pass the syntactic check
    // even though the real, on-disk location is out of bounds. Resolve as
    // much of the path as already exists to its real location (following
    // reparse points), re-append any not-yet-existing tail, and re-check
    // that real location too.
    //
    // This does not close every gap: a reparse point created *after* this
    // check but *before* the actual file I/O (TOCTOU) is out of scope for a
    // check-then-open design without OS-level no-follow opens, and is
    // documented in docs/SECURITY_NOTES.md.
    match resolve_real_path(&normalized) {
        // `canonicalize` commonly returns the Windows extended-length
        // (`\\?\C:\...`) form, so route it back through `normalize_path` to
        // fold that back to the same representation used everywhere else
        // before comparing it against the workspace patterns.
        Some(real) => match normalize_path(&real) {
            Ok(real_normalized) => is_path_allowed_syntactic(&real_normalized, config),
            Err(_) => false,
        },
        None => true,
    }
}

/// Pure syntactic allow/deny evaluation (no filesystem access): normalizes
/// `path` into case-folded, separator-agnostic segments and matches them
/// against every deny pattern (deny wins) and then the allow list.
fn is_path_allowed_syntactic(normalized: &Path, config: &WorkspaceConfig) -> bool {
    let candidate = path_segments_for_match(normalized);

    for deny in &config.deny {
        if pattern_matches(deny, &candidate) {
            return false;
        }
    }

    if config.allow.is_empty() {
        return false;
    }

    config
        .allow
        .iter()
        .any(|allow| pattern_matches(allow, &candidate))
}

/// Walk up from `path` until an ancestor that actually exists on disk is
/// found (or the path is exhausted). Used to detect symlink/junction
/// escapes even when the final path component does not exist yet (e.g. a
/// brand-new file being written through an already-existing, possibly
/// reparsed, parent directory).
fn nearest_existing_ancestor(path: &Path) -> Option<PathBuf> {
    let mut current = path.to_path_buf();
    loop {
        if current.exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

/// Resolve `path` to its real, reparse-point-free location as far as
/// possible: canonicalize the nearest *existing* ancestor (which follows
/// any symlink/junction along the way), then re-append whatever tail of
/// `path` does not exist yet. Returns `None` when nothing along the path
/// exists, or canonicalization fails, in which case the syntactic
/// normalization is trusted as-is.
///
/// Reconstructing ancestor-plus-tail (rather than checking the ancestor by
/// itself) matters even when nothing is a symlink: if the allowed
/// workspace root itself does not exist on disk yet, the nearest existing
/// ancestor may be a *shorter* real path than the workspace root (e.g. the
/// drive root), which would otherwise be judged "not under the workspace"
/// even though the requested path, once its missing directories are
/// created, legitimately would be.
fn resolve_real_path(path: &Path) -> Option<PathBuf> {
    let existing = nearest_existing_ancestor(path)?;
    let tail = path.strip_prefix(&existing).ok()?;
    let mut real = existing.canonicalize().ok()?;
    real.push(tail);
    Some(real)
}

pub fn resolve_and_check_path(
    path: &Path,
    working_dir: &Path,
    config: &WorkspaceConfig,
) -> Result<PathBuf, SandboxError> {
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        working_dir.join(path)
    };

    let normalized =
        normalize_path(&resolved).map_err(|e| SandboxError::ResolutionFailed(e.to_string()))?;

    if !is_path_allowed(&normalized, config) {
        return Err(SandboxError::PathNotAllowed(normalized));
    }

    Ok(normalized)
}

pub fn resolve_and_check(
    path: &Path,
    working_dir: &Path,
    config: &WorkspaceConfig,
) -> Result<PathBuf, SandboxError> {
    resolve_and_check_path(path, working_dir, config)
}

/// Windows device names that are reserved regardless of directory or
/// extension (`CON`, `CON.txt`, `con`, ... all resolve to the same device,
/// never an ordinary file).
const RESERVED_DEVICE_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

fn is_reserved_device_name(segment: &str) -> bool {
    let base = segment.split('.').next().unwrap_or(segment);
    RESERVED_DEVICE_NAMES
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(base))
}

fn disk_prefix_string(letter: u8) -> String {
    format!("{}:", letter as char)
}

fn unc_prefix_string(server: &std::ffi::OsStr, share: &std::ffi::OsStr) -> std::ffi::OsString {
    let mut out = std::ffi::OsString::from(r"\\");
    out.push(server);
    out.push("\\");
    out.push(share);
    out
}

/// Syntactically normalize a path: expand `~`/env vars, resolve `.`/`..`
/// components (without ever touching the filesystem), fold Windows prefix
/// forms (drive letter, UNC, extended-length `\\?\`) to one consistent
/// representation, and reject constructs that either don't represent an
/// ordinary workspace file or could be used to disguise one:
///   - `\\.\...` device paths (raw volume/device handles).
///   - NTFS alternate data stream references (`name:stream`).
///   - Reserved Windows device names (`CON`, `NUL`, `COM1`, ...), which
///     resolve to a device rather than a file regardless of directory.
///
/// Trailing dots/spaces on a path segment are also stripped, mirroring how
/// the Windows API itself resolves such names, so a deny pattern for
/// `secret.key` cannot be dodged with `secret.key.` or `secret.key `.
fn normalize_path(path: &Path) -> Result<PathBuf, SandboxError> {
    let raw = path.to_string_lossy();
    let expanded = shellexpand::tilde(&raw).into_owned();
    let expanded = shellexpand::env(&expanded)
        .map(|v| v.into_owned())
        .unwrap_or(expanded);

    let mut out = PathBuf::new();

    for component in Path::new(&expanded).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Prefix(prefix) => match prefix.kind() {
                Prefix::DeviceNS(_) => {
                    return Err(SandboxError::ResolutionFailed(
                        "device paths (\\\\.\\...) are not allowed".into(),
                    ));
                }
                Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
                    out.push(disk_prefix_string(letter));
                }
                Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
                    out.push(unc_prefix_string(server, share));
                }
                Prefix::Verbatim(_) => {
                    // Raw verbatim volume paths (\\?\Volume{GUID}\...) never
                    // correspond to a configured drive-letter/UNC workspace
                    // pattern; push through unnormalized so they simply
                    // fail to match anything (fail-closed via "no match").
                    out.push(prefix.as_os_str());
                }
            },
            Component::Normal(segment) => {
                let segment = segment.to_string_lossy();
                if segment.contains(':') {
                    return Err(SandboxError::ResolutionFailed(format!(
                        "'{segment}' looks like an NTFS alternate data stream reference, which is not allowed"
                    )));
                }
                let trimmed = segment.trim_end_matches(['.', ' ']);
                let trimmed = if trimmed.is_empty() {
                    segment.as_ref()
                } else {
                    trimmed
                };
                if is_reserved_device_name(trimmed) {
                    return Err(SandboxError::ResolutionFailed(format!(
                        "'{trimmed}' is a reserved Windows device name"
                    )));
                }
                out.push(trimmed);
            }
            other => out.push(other.as_os_str()),
        }
    }

    Ok(out)
}

/// Windows filesystem paths are case-insensitive; fold to ASCII-lowercase
/// purely for workspace boundary comparisons (the path returned by
/// `normalize_path`/`resolve_and_check_path` for actual I/O keeps its
/// original case). Not folded on non-Windows targets, where filesystems are
/// typically case-sensitive.
#[cfg(windows)]
fn fold_case(s: &str) -> String {
    s.to_ascii_lowercase()
}

#[cfg(not(windows))]
fn fold_case(s: &str) -> String {
    s.to_string()
}

/// Split an already-`normalize_path`d path into case-folded, separator-
/// agnostic segments for comparison against a parsed pattern.
fn path_segments_for_match(normalized: &Path) -> Vec<String> {
    normalized
        .to_string_lossy()
        .split(['/', '\\'])
        .filter(|s| !s.is_empty())
        .map(fold_case)
        .collect()
}

/// Parse a workspace allow/deny pattern into normalized, case-folded path
/// segments, plus whether it is an "anywhere" pattern (a leading `**`
/// segment) that may match starting at any component boundary rather than
/// only at the path root. A trailing `**` segment (e.g. `**/target/**`) is
/// dropped: matching the directory component itself already implies
/// everything nested below it for this containment-style check.
fn parse_pattern(pattern: &str) -> (bool, Vec<String>) {
    let expanded = shellexpand::tilde(pattern).into_owned();
    let expanded = shellexpand::env(&expanded)
        .map(|v| v.into_owned())
        .unwrap_or(expanded);

    let mut segments: Vec<String> = expanded
        .split(['/', '\\'])
        .filter(|s| !s.is_empty())
        .map(fold_case)
        .collect();

    let anywhere = segments.first().map(|s| s.as_str()) == Some("**");
    if anywhere {
        segments.remove(0);
    }
    if segments.last().map(|s| s.as_str()) == Some("**") {
        segments.pop();
    }

    (anywhere, segments)
}

fn starts_with_segments(path: &[String], prefix: &[String]) -> bool {
    prefix.len() <= path.len() && path[..prefix.len()] == prefix[..]
}

fn contains_subsequence(path: &[String], needle: &[String]) -> bool {
    if needle.is_empty() || needle.len() > path.len() {
        return false;
    }
    path.windows(needle.len()).any(|w| w == needle)
}

fn pattern_matches(pattern: &str, candidate: &[String]) -> bool {
    let (anywhere, segments) = parse_pattern(pattern);
    if segments.is_empty() {
        return false;
    }
    if anywhere {
        contains_subsequence(candidate, &segments)
    } else {
        starts_with_segments(candidate, &segments)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_config::WorkspaceConfig;
    use tempfile::tempdir;

    #[test]
    fn test_is_path_allowed_allowed() {
        let dir = tempdir().unwrap();
        let path = dir.path();

        let config = WorkspaceConfig {
            allow: vec![path.to_string_lossy().to_string()],
            deny: vec![],
        };

        assert!(is_path_allowed(path, &config));
    }

    #[test]
    fn test_is_path_allowed_denied() {
        let dir = tempdir().unwrap();
        let path = dir.path();

        let config = WorkspaceConfig {
            allow: vec![path.to_string_lossy().to_string()],
            deny: vec![path.to_string_lossy().to_string()],
        };

        assert!(!is_path_allowed(path, &config));
    }

    #[test]
    fn test_is_path_allowed_path_traversal() {
        let dir = tempdir().unwrap();
        let allowed = dir.path().join("allowed");
        let denied = dir.path().join("denied");
        std::fs::create_dir_all(&allowed).unwrap();
        std::fs::create_dir_all(&denied).unwrap();

        let config = WorkspaceConfig {
            allow: vec![allowed.to_string_lossy().to_string()],
            deny: vec![],
        };

        // Try to traverse from allowed to denied
        let traversal = allowed.join("../denied");
        assert!(!is_path_allowed(&traversal, &config));
    }

    #[test]
    fn test_is_path_allowed_empty_allow_list() {
        let dir = tempdir().unwrap();
        let path = dir.path();

        let config = WorkspaceConfig {
            allow: vec![],
            deny: vec![],
        };

        assert!(!is_path_allowed(path, &config));
    }

    #[test]
    fn test_is_path_allowed_subdirectory() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let subdir = root.join("subdir");
        std::fs::create_dir_all(&subdir).unwrap();

        let config = WorkspaceConfig {
            allow: vec![root.to_string_lossy().to_string()],
            deny: vec![],
        };

        assert!(is_path_allowed(&subdir, &config));
    }

    #[test]
    fn test_resolve_and_check_path_absolute() {
        let dir = tempdir().unwrap();
        let path = dir.path();

        let config = WorkspaceConfig {
            allow: vec![path.to_string_lossy().to_string()],
            deny: vec![],
        };

        let result = resolve_and_check_path(path, path, &config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_resolve_and_check_path_relative() {
        let dir = tempdir().unwrap();
        let path = dir.path();
        std::fs::create_dir_all(path.join("subdir")).unwrap();

        let config = WorkspaceConfig {
            allow: vec![path.to_string_lossy().to_string()],
            deny: vec![],
        };

        let result = resolve_and_check_path(Path::new("subdir"), path, &config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), path.join("subdir"));
    }

    #[test]
    fn test_resolve_and_check_path_traversal_blocked() {
        let dir = tempdir().unwrap();
        let path = dir.path();
        let subdir = path.join("subdir");
        std::fs::create_dir_all(&subdir).unwrap();

        let config = WorkspaceConfig {
            allow: vec![subdir.to_string_lossy().to_string()],
            deny: vec![],
        };

        // Try to escape from subdir to parent
        let result = resolve_and_check_path(Path::new("../"), &subdir, &config);
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_and_check_path_denied() {
        let dir = tempdir().unwrap();
        let path = dir.path();

        let config = WorkspaceConfig {
            allow: vec![path.to_string_lossy().to_string()],
            deny: vec![path.to_string_lossy().to_string()],
        };

        let result = resolve_and_check_path(path, path, &config);
        assert!(result.is_err());
    }

    #[test]
    fn test_normalize_path_removes_curdir() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("foo").join(".").join("bar");

        let result = normalize_path(&path);
        assert!(result.is_ok());
        let normalized = result.unwrap();
        assert_eq!(normalized, dir.path().join("foo").join("bar"));
    }

    #[test]
    fn test_normalize_path_removes_parent_dir() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("foo").join("..").join("bar");

        let result = normalize_path(&path);
        assert!(result.is_ok());
        let normalized = result.unwrap();
        assert_eq!(normalized, dir.path().join("bar"));
    }

    #[test]
    fn test_normalize_path_multiple_traversals() {
        let dir = tempdir().unwrap();
        let path = dir
            .path()
            .join("a")
            .join("b")
            .join("..")
            .join("..")
            .join("c");

        let result = normalize_path(&path);
        assert!(result.is_ok());
        let normalized = result.unwrap();
        assert_eq!(normalized, dir.path().join("c"));
    }

    // =======================================================================
    // Phase 3: Windows workspace boundary hardening
    // =======================================================================

    #[test]
    fn test_drive_letter_mismatch_is_denied() {
        let config = WorkspaceConfig {
            allow: vec![r"C:\Workspace".to_string()],
            deny: vec![],
        };
        assert!(!is_path_allowed(
            Path::new(r"D:\Workspace\file.txt"),
            &config
        ));
        assert!(is_path_allowed(
            Path::new(r"C:\Workspace\file.txt"),
            &config
        ));
    }

    #[test]
    fn test_case_insensitive_workspace_match() {
        let config = WorkspaceConfig {
            allow: vec![r"C:\Workspace".to_string()],
            deny: vec![],
        };
        assert!(is_path_allowed(
            Path::new(r"c:\workspace\file.txt"),
            &config
        ));
        assert!(is_path_allowed(
            Path::new(r"C:\WORKSPACE\FILE.TXT"),
            &config
        ));
    }

    #[test]
    fn test_case_insensitive_deny_cannot_be_bypassed_by_case() {
        let config = WorkspaceConfig {
            allow: vec![r"C:\Workspace".to_string()],
            deny: vec![r"C:\Workspace\Secret".to_string()],
        };
        assert!(!is_path_allowed(
            Path::new(r"c:\WORKSPACE\secret\id_rsa"),
            &config
        ));
    }

    #[test]
    fn test_mixed_separators_match_same_as_backslash() {
        let config = WorkspaceConfig {
            allow: vec![r"C:\Workspace".to_string()],
            deny: vec![],
        };
        assert!(is_path_allowed(
            Path::new("C:/Workspace/dir/file.txt"),
            &config
        ));
        assert!(is_path_allowed(
            Path::new(r"C:/Workspace\dir/file.txt"),
            &config
        ));
    }

    #[test]
    fn test_traversal_cannot_escape_allowed_root() {
        let config = WorkspaceConfig {
            allow: vec![r"C:\Workspace".to_string()],
            deny: vec![],
        };
        assert!(!is_path_allowed(
            Path::new(r"C:\Workspace\..\Other\file.txt"),
            &config
        ));
    }

    #[test]
    fn test_traversal_past_drive_root_clamps_at_root_without_panicking() {
        // Popping past the root is a no-op (PathBuf::pop returns false when
        // there is no parent), so extra ".." segments beyond the drive root
        // are absorbed rather than escaping to some other location.
        let result = normalize_path(Path::new(r"C:\..\..\..\evil"));
        assert_eq!(result.unwrap(), PathBuf::from(r"C:\evil"));
    }

    #[test]
    fn test_traversal_past_drive_root_cannot_escape_to_unintended_allow_match() {
        let config = WorkspaceConfig {
            allow: vec![r"C:\Workspace".to_string()],
            deny: vec![],
        };
        assert!(!is_path_allowed(Path::new(r"C:\..\..\..\evil"), &config));
    }

    #[test]
    fn test_absolute_path_outside_any_allowed_root_is_denied() {
        let config = WorkspaceConfig {
            allow: vec![r"C:\Workspace".to_string()],
            deny: vec![],
        };
        assert!(!is_path_allowed(
            Path::new(r"C:\Elsewhere\file.txt"),
            &config
        ));
    }

    #[test]
    fn test_workspace_prefix_confusion_is_not_authorized() {
        // C:\allowed must NOT authorize C:\allowed-evil: our segment-based
        // comparison (like `Path::starts_with`) matches whole components,
        // never a raw string prefix.
        let config = WorkspaceConfig {
            allow: vec![r"C:\allowed".to_string()],
            deny: vec![],
        };
        assert!(!is_path_allowed(
            Path::new(r"C:\allowed-evil\file.txt"),
            &config
        ));
        assert!(is_path_allowed(Path::new(r"C:\allowed\file.txt"), &config));
    }

    #[test]
    fn test_unc_path_allowed_under_matching_share_only() {
        let config = WorkspaceConfig {
            allow: vec![r"\\server\share".to_string()],
            deny: vec![],
        };
        assert!(is_path_allowed(
            Path::new(r"\\server\share\dir\file.txt"),
            &config
        ));
        assert!(!is_path_allowed(
            Path::new(r"\\server\othershare\file.txt"),
            &config
        ));
        assert!(!is_path_allowed(
            Path::new(r"\\otherserver\share\file.txt"),
            &config
        ));
    }

    #[test]
    fn test_extended_length_verbatim_disk_equivalent_to_plain_form() {
        let config = WorkspaceConfig {
            allow: vec![r"C:\Workspace".to_string()],
            deny: vec![],
        };
        assert!(is_path_allowed(
            Path::new(r"\\?\C:\Workspace\file.txt"),
            &config
        ));
    }

    #[test]
    fn test_extended_length_verbatim_unc_equivalent_to_plain_form() {
        let config = WorkspaceConfig {
            allow: vec![r"\\server\share".to_string()],
            deny: vec![],
        };
        assert!(is_path_allowed(
            Path::new(r"\\?\UNC\server\share\dir\file.txt"),
            &config
        ));
    }

    #[test]
    fn test_device_path_always_denied_even_with_broad_allow() {
        let config = WorkspaceConfig {
            allow: vec![r"C:\".to_string()],
            deny: vec![],
        };
        assert!(!is_path_allowed(Path::new(r"\\.\C:\Workspace"), &config));
        assert!(!is_path_allowed(Path::new(r"\\.\PhysicalDrive0"), &config));
    }

    #[test]
    fn test_trailing_dot_and_space_cannot_bypass_deny_pattern() {
        let config = WorkspaceConfig {
            allow: vec![r"C:\Workspace".to_string()],
            deny: vec!["**/secret.key".to_string()],
        };
        assert!(!is_path_allowed(
            Path::new(r"C:\Workspace\secret.key"),
            &config
        ));
        assert!(!is_path_allowed(
            Path::new(r"C:\Workspace\secret.key."),
            &config
        ));
        assert!(!is_path_allowed(
            Path::new(r"C:\Workspace\secret.key "),
            &config
        ));
        assert!(!is_path_allowed(
            Path::new(r"C:\Workspace\secret.key..."),
            &config
        ));
    }

    #[test]
    fn test_reserved_device_names_are_denied() {
        let config = WorkspaceConfig {
            allow: vec![r"C:\Workspace".to_string()],
            deny: vec![],
        };
        assert!(!is_path_allowed(Path::new(r"C:\Workspace\CON"), &config));
        assert!(!is_path_allowed(Path::new(r"C:\Workspace\con"), &config));
        assert!(!is_path_allowed(
            Path::new(r"C:\Workspace\CON.txt"),
            &config
        ));
        assert!(!is_path_allowed(Path::new(r"C:\Workspace\NUL"), &config));
        assert!(!is_path_allowed(Path::new(r"C:\Workspace\COM1"), &config));
        assert!(!is_path_allowed(Path::new(r"C:\Workspace\LPT1"), &config));
        // Not a reserved name: "CONSOLE" is not "CON".
        assert!(is_path_allowed(
            Path::new(r"C:\Workspace\CONSOLE.txt"),
            &config
        ));
    }

    #[test]
    fn test_git_and_target_boundaries_denied_anywhere_under_workspace() {
        let config = WorkspaceConfig {
            allow: vec![r"C:\Workspace".to_string()],
            deny: vec![
                "**/.git/**".to_string(),
                "**/target/**".to_string(),
                "**/node_modules/**".to_string(),
            ],
        };
        assert!(!is_path_allowed(
            Path::new(r"C:\Workspace\repo\.git\config"),
            &config
        ));
        assert!(!is_path_allowed(
            Path::new(r"C:\Workspace\project\target\debug\build\out"),
            &config
        ));
        assert!(!is_path_allowed(
            Path::new(r"C:\Workspace\project\node_modules\pkg\index.js"),
            &config
        ));
        // An ordinary source file must still be allowed.
        assert!(is_path_allowed(
            Path::new(r"C:\Workspace\src\main.rs"),
            &config
        ));
    }

    #[test]
    fn test_alternate_data_stream_reference_is_denied() {
        let config = WorkspaceConfig {
            allow: vec![r"C:\Workspace".to_string()],
            deny: vec![],
        };
        assert!(!is_path_allowed(
            Path::new(r"C:\Workspace\secret.key:$DATA"),
            &config
        ));
        assert!(!is_path_allowed(
            Path::new(r"C:\Workspace\notes.txt:hidden"),
            &config
        ));
    }

    #[test]
    fn test_nearest_existing_ancestor_walks_up_to_closest_real_directory() {
        // Deterministic, privilege-free coverage of the mechanism the
        // symlink/junction defense (below) relies on: when the target
        // itself does not exist yet, the nearest *existing* ancestor is
        // found so it can be canonicalized and re-checked.
        let dir = tempdir().unwrap();
        let existing = dir.path().join("existing");
        std::fs::create_dir_all(&existing).unwrap();

        let missing = existing.join("not_yet_created").join("file.txt");
        assert_eq!(nearest_existing_ancestor(&missing), Some(existing.clone()));
        assert_eq!(nearest_existing_ancestor(&existing), Some(existing));
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires SeCreateSymbolicLinkPrivilege or Windows Developer \
                Mode to create a real directory symlink; run manually with \
                `cargo test -p sc-sandbox -- --ignored` on a machine that \
                has it. Verified passing on the implementation machine \
                (2026-07-14) but not assumed available in every environment \
                (e.g. a locked-down CI runner)."]
    fn test_live_symlink_escape_is_denied_via_canonicalized_ancestor() {
        let dir = tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), b"top secret").unwrap();

        let link = workspace.join("escape");
        std::os::windows::fs::symlink_dir(&outside, &link)
            .expect("symlink_dir failed; see #[ignore] reason on this test");

        let config = WorkspaceConfig {
            allow: vec![workspace.to_string_lossy().to_string()],
            deny: vec![],
        };

        // Syntactically this looks like an ordinary path under the allowed
        // workspace, but `escape` is a symlink pointing outside it.
        assert!(
            !is_path_allowed(&link.join("secret.txt"), &config),
            "a symlink inside the workspace pointing outside it must not grant access"
        );

        // A brand-new (not-yet-existing) file written through the same
        // symlinked directory must also be denied: the directory (the
        // symlink itself) already exists and resolves outside the
        // workspace, even though the file does not exist yet.
        assert!(
            !is_path_allowed(&link.join("new_file.txt"), &config),
            "writing a new file through a symlinked escape must not be allowed either"
        );
    }
}
