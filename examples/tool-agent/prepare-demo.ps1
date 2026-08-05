param(
    [string]$Model = "llama3.2:3b",
    [string]$BinaryPath = ""
)

$ErrorActionPreference = "Stop"

if (-not $BinaryPath) {
    $BinaryPath = Join-Path $PSScriptRoot "..\..\target\release\sc-agent.exe"
}
$BinaryPath = [System.IO.Path]::GetFullPath($BinaryPath)
if (-not (Test-Path $BinaryPath -PathType Leaf)) {
    throw "SC Node binary not found at '$BinaryPath'. Run 'cargo build --release --locked' first."
}

$demoRoot = Join-Path $env:TEMP "sc-node-tool-agent-demo"
$workspace = Join-Path $demoRoot "workspace"
$dataDir = Join-Path $demoRoot "data"
$configPath = Join-Path $demoRoot "config.toml"

New-Item -ItemType Directory -Force -Path $workspace, $dataDir | Out-Null

$notes = @"
SC Node tool-agent demo

Tasks:
- Summarize this file in three bullet points.
- Save the result as summary.md in this workspace.
- Calculate the SHA-256 hash of summary.md with a shell tool.
"@
[System.IO.File]::WriteAllText(
    (Join-Path $workspace "notes.txt"),
    $notes,
    [System.Text.UTF8Encoding]::new($false)
)
[System.IO.File]::WriteAllText(
    (Join-Path $workspace "blocked.secret"),
    "This harmless fixture must be denied by the demo policy.",
    [System.Text.UTF8Encoding]::new($false)
)

$workspaceToml = $workspace.Replace("\", "/")
$dataDirToml = $dataDir.Replace("\", "/")

$config = @"
[general]
log_level = "info"
data_dir = "$dataDirToml"
no_telemetry = true

[workspace]
allow = ["$workspaceToml"]
deny = ["**/.git/**", "**/.env*", "**/*.secret"]

[permissions]
default_policy = "deny"

[permissions.tools.file]
policy = "ask"
allow_patterns = ["*.md", "*.txt", "*.json"]
deny_patterns = ["*.secret", "credentials*", "*.key", "*.pem"]

[permissions.tools.shell]
policy = "ask"
allow_patterns = ["powershell.exe ", "pwsh ", "cmd.exe "]
deny_patterns = ["Remove-Item -Recurse -Force", "del /s /q", "format ", "shutdown", "reboot"]

[providers.ollama]
enabled = true
base_url = "http://127.0.0.1:11434"
default_model = "$Model"
keep_alive = "5m"
timeout_secs = 120
max_retries = 2

[providers.openrouter]
enabled = false

[providers.nvidia]
enabled = false

[routing]
rules = []
fallback_provider = "ollama"
fallback_model = "$Model"

[audit]
enabled = true
path = "audit.jsonl"
max_size_mb = 10
max_files = 2
log_args = true
log_output = true
"@
[System.IO.File]::WriteAllText(
    $configPath,
    $config,
    [System.Text.UTF8Encoding]::new($false)
)

Write-Output "DEMO_ROOT=$demoRoot"
Write-Output "WORKSPACE=$workspace"
Write-Output "CONFIG=$configPath"
Write-Output "BINARY=$BinaryPath"
Write-Output "MODEL=$Model"
Write-Output ""
Write-Output "For this PowerShell session run:"
Write-Output ('$env:SC_AGENT_CONFIG = "' + $configPath + '"')
Write-Output ('& "' + $BinaryPath + '" doctor')
Write-Output ('& "' + $BinaryPath + '" repl')
Write-Output ""
Write-Output "The demo is isolated under the Windows temporary directory and does not modify your normal SC Node profile."
