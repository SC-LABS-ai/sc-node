$ErrorActionPreference = "Stop"

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot "..\.."))
Push-Location $repoRoot
try {
    $checks = @(
        @("Denied shell call never executes", @("test", "-p", "sc-agent-core", "test_gate_1_denied_shell_by_policy_never_executes")),
        @("Ask auto-denies without an interactive approver", @("test", "-p", "sc-agent-core", "test_gate_4_ask_under_auto_deny_fails_closed")),
        @("Workspace traversal is blocked", @("test", "-p", "sc-sandbox", "test_resolve_and_check_path_traversal_blocked")),
        @("Sensitive file pattern is blocked", @("test", "-p", "sc-tool-file", "test_read_file_execute_denies_secret_pattern"))
    )

    foreach ($check in $checks) {
        $name = $check[0]
        $arguments = $check[1]
        Write-Host "[RUN] $name"
        & cargo @arguments
        if ($LASTEXITCODE -ne 0) {
            throw "Guard verification failed: $name"
        }
        Write-Host "[PASS] $name"
    }

    Write-Host ""
    Write-Host "RESULT: 4 deterministic guard checks passed."
}
finally {
    Pop-Location
}
