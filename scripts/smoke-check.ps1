# SC Node smoke check
# Cross-platform local health check. No API keys or live model provider required.
# Verifies: cargo check, cargo test, --help, --version, and isolated config init.

$ErrorActionPreference = "Stop"

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("sc-node-smoke-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $tempRoot | Out-Null

function Invoke-NativeStep {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Executable,
        [Parameter(Mandatory = $true)][string[]]$Arguments,
        [Parameter(Mandatory = $true)][string]$OutputFile
    )

    Write-Host ""
    Write-Host $Name -ForegroundColor Yellow

    if (Test-Path $OutputFile) {
        Remove-Item $OutputFile -Force
    }

    $previousErrorActionPreference = $ErrorActionPreference
    try {
        # Windows PowerShell 5.1 wraps native stderr as NativeCommandError when
        # ErrorActionPreference is Stop, even when the process exits successfully.
        # Capture the process streams first and decide solely from its exit code.
        $ErrorActionPreference = "Continue"
        & $Executable @Arguments *> $OutputFile
        $exitCode = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $previousErrorActionPreference
    }
    $output = if (Test-Path $OutputFile) { Get-Content $OutputFile -Raw } else { "" }

    if ($exitCode -ne 0) {
        Write-Host "FAIL: $Name (exit $exitCode)" -ForegroundColor Red
        Write-Host $output
        throw "$Name failed with exit code $exitCode"
    }

    if ($output -match "warning:") {
        Write-Host "WARN: $Name completed with warnings" -ForegroundColor Yellow
        $output -split "`r?`n" |
            Select-String "warning:" |
            Select-Object -First 10 |
            ForEach-Object { Write-Host $_ }
    } else {
        Write-Host "PASS: $Name" -ForegroundColor Green
    }

    return $output
}

Push-Location $repoRoot
try {
    Write-Host ""
    Write-Host "==================================================" -ForegroundColor Cyan
    Write-Host " SC NODE SMOKE CHECK" -ForegroundColor Cyan
    Write-Host "==================================================" -ForegroundColor Cyan

    Invoke-NativeStep `
        -Name "[1/5] cargo check --workspace --locked" `
        -Executable "cargo" `
        -Arguments @("check", "--workspace", "--locked") `
        -OutputFile (Join-Path $tempRoot "check.txt") | Out-Null

    Invoke-NativeStep `
        -Name "[2/5] cargo test --workspace --locked" `
        -Executable "cargo" `
        -Arguments @("test", "--workspace", "--locked") `
        -OutputFile (Join-Path $tempRoot "test.txt") | Out-Null

    $help = Invoke-NativeStep `
        -Name "[3/5] sc-agent --help" `
        -Executable "cargo" `
        -Arguments @("run", "--locked", "--", "--help") `
        -OutputFile (Join-Path $tempRoot "help.txt")
    if ($help -notmatch "run" -or $help -notmatch "repl") {
        throw "--help output is missing expected subcommands"
    }

    $version = Invoke-NativeStep `
        -Name "[4/5] sc-agent --version" `
        -Executable "cargo" `
        -Arguments @("run", "--locked", "--", "--version") `
        -OutputFile (Join-Path $tempRoot "version.txt")
    if ($version -notmatch "sc-agent\s+\d+\.\d+\.\d+") {
        throw "--version output is unexpected: $version"
    }

    Write-Host ""
    Write-Host "[5/5] sc-agent config-init (isolated SC_AGENT_CONFIG)" -ForegroundColor Yellow
    $configPath = Join-Path $tempRoot "profile\config.toml"
    $oldConfig = $env:SC_AGENT_CONFIG
    try {
        $env:SC_AGENT_CONFIG = $configPath
        $init = Invoke-NativeStep `
            -Name "sc-agent config-init" `
            -Executable "cargo" `
            -Arguments @("run", "--locked", "--", "config-init") `
            -OutputFile (Join-Path $tempRoot "init.txt")
        if ($init -notmatch "Created default config") {
            throw "config-init output is unexpected: $init"
        }
        if (-not (Test-Path $configPath -PathType Leaf)) {
            throw "config-init did not create the isolated config file"
        }
        Write-Host "PASS: isolated config created at $configPath" -ForegroundColor Green
    }
    finally {
        if ($null -eq $oldConfig) {
            Remove-Item Env:SC_AGENT_CONFIG -ErrorAction SilentlyContinue
        } else {
            $env:SC_AGENT_CONFIG = $oldConfig
        }
    }

    Write-Host ""
    Write-Host "==================================================" -ForegroundColor Green
    Write-Host " SC NODE SMOKE CHECK PASSED" -ForegroundColor Green
    Write-Host "==================================================" -ForegroundColor Green
}
finally {
    Pop-Location
    if (Test-Path $tempRoot) {
        Remove-Item -Recurse -Force $tempRoot
    }
}
