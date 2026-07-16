# SC Node Smoke Check
# Safe local health check. No API keys required. No Ollama required.
# Verifies: cargo check, cargo test, --help, --version, init (isolated).

$ErrorActionPreference = "Stop"

Push-Location (Join-Path $PSScriptRoot "..")

function Invoke-CmdStep {
    param([string]$Name, [string]$Command, [string]$OutputFile)

    Write-Host ""
    Write-Host $Name -ForegroundColor Yellow

    Remove-Item $OutputFile -Force -ErrorAction SilentlyContinue

    cmd /c "$Command > `"$OutputFile`" 2>&1"
    $exitCode = $LASTEXITCODE

    $output = ""
    if (Test-Path $OutputFile) { $output = Get-Content $OutputFile -Raw }

    if ($exitCode -ne 0) {
        Write-Host "FAIL: $Name (exit $exitCode)" -ForegroundColor Red
        Write-Host $output
        exit $exitCode
    }

    if ($output -match "warning:") {
        Write-Host "WARN: $Name completed with warnings" -ForegroundColor Yellow
        $output -split "`r?`n" | Select-String "warning:" | Select-Object -First 10 | ForEach-Object { Write-Host $_ }
    } else {
        Write-Host "PASS: $Name" -ForegroundColor Green
    }
    return $output
}

try {
    Write-Host ""
    Write-Host "==================================================" -ForegroundColor Cyan
    Write-Host " SC NODE SMOKE CHECK" -ForegroundColor Cyan
    Write-Host "==================================================" -ForegroundColor Cyan

    # ── Build & Test ─────────────────────────────────────────
    Invoke-CmdStep "[1/5] cargo check" "cargo check" ".smoke-check.txt" | Out-Null
    Invoke-CmdStep "[2/5] cargo test --workspace" "cargo test --workspace" ".smoke-test.txt" | Out-Null

    # ── CLI bootstrap (no config needed) ─────────────────────
    $help = Invoke-CmdStep "[3/5] sc-agent --help" "cargo run -- --help" ".smoke-help.txt"
    if ($help -notmatch "run" -or $help -notmatch "repl") {
        Write-Host "FAIL: --help output missing expected subcommands" -ForegroundColor Red
        exit 1
    }

    $ver = Invoke-CmdStep "[4/5] sc-agent --version" "cargo run -- --version" ".smoke-version.txt"
    if ($ver -notmatch "sc-agent \d+\.\d+\.\d+") {
        Write-Host "FAIL: --version output unexpected" -ForegroundColor Red
        exit 1
    }

    # ── sc-agent init (isolated via temp HOME) ───────────────
    $tempHome = Join-Path $env:TEMP "sc-agent-smoke-home"
    Remove-Item -Recurse -Force $tempHome -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $tempHome | Out-Null

    Write-Host ""
    Write-Host "[5/5] sc-agent init (temp HOME=$tempHome)" -ForegroundColor Yellow
    $oldHome = $env:USERPROFILE
    try {
        $env:USERPROFILE = $tempHome
        # Preserve Rust toolchain paths so cargo still works
        $env:RUSTUP_HOME = Join-Path $oldHome ".rustup"
        $env:CARGO_HOME = Join-Path $oldHome ".cargo"
        $init = cmd /c "cargo run -- config-init > .smoke-init.txt 2>&1"
        $exitCode = $LASTEXITCODE
        $initOut = Get-Content ".smoke-init.txt" -Raw -ErrorAction SilentlyContinue

        if ($exitCode -ne 0) {
            Write-Host "FAIL: init crashed" -ForegroundColor Red
            Write-Host $initOut
            exit $exitCode
        }
        if ($initOut -match "Created default config") {
            Write-Host "PASS: sc-agent config-init (isolated)" -ForegroundColor Green
        } else {
            Write-Host "WARN: init ran but output unexpected:" -ForegroundColor Yellow
            Write-Host $initOut
        }
    } finally {
        $env:USERPROFILE = $oldHome
        Remove-Item -Recurse -Force $tempHome -ErrorAction SilentlyContinue
    }

    # ── Cleanup ──────────────────────────────────────────────
    Remove-Item ".smoke-*.txt" -Force -ErrorAction SilentlyContinue

    Write-Host ""
    Write-Host "==================================================" -ForegroundColor Green
    Write-Host " SC NODE SMOKE CHECK PASSED" -ForegroundColor Green
    Write-Host "==================================================" -ForegroundColor Green
}
finally {
    Pop-Location
}