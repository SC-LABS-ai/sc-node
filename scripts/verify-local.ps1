# SC Node Full Local Verify
# Safe verification script for local development.
# Does not modify repo files or real user config.

$ErrorActionPreference = "Stop"

# Resolve project root from script location
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$project = Split-Path -Parent $scriptDir

function Invoke-CmdStep {
    param(
        [string]$Name,
        [string]$Command,
        [string]$OutputFile
    )

    Write-Host ""
    Write-Host $Name -ForegroundColor Yellow

    Remove-Item $OutputFile -Force -ErrorAction SilentlyContinue

    cmd /c "$Command > `"$OutputFile`" 2>&1"
    $exitCode = $LASTEXITCODE

    $output = ""
    if (Test-Path $OutputFile) {
        $output = Get-Content $OutputFile -Raw
    }

    if ($exitCode -ne 0) {
        Write-Host "FAIL: $Name" -ForegroundColor Red
        Write-Host $output
        exit $exitCode
    }

    if ($output -match "warning:") {
        Write-Host "WARN: $Name completed with warnings" -ForegroundColor Yellow
        $output -split "`r?`n" | Select-String "warning:" | Select-Object -First 10 | ForEach-Object {
            Write-Host $_
        }
    } else {
        Write-Host "PASS: $Name" -ForegroundColor Green
    }

    return $output
}

Write-Host ""
Write-Host "==================================================" -ForegroundColor Cyan
Write-Host " SC NODE / sc-agent - FULL LOCAL VERIFY" -ForegroundColor Cyan
Write-Host "==================================================" -ForegroundColor Cyan
Write-Host ""

if (-not (Test-Path $project)) {
    throw "Project folder not found: $project"
}

Set-Location $project

Remove-Item ".smoke-*.txt",".verify-*.txt" -Force -ErrorAction SilentlyContinue

Write-Host ""
Write-Host "[1/8] Git status" -ForegroundColor Yellow
git status
if ($LASTEXITCODE -ne 0) { throw "git status failed" }

$dirty = git status --porcelain
if ($dirty) {
    Write-Host "Repo is NOT clean:" -ForegroundColor Red
    Write-Host $dirty
    throw "Working tree is dirty. Stop."
}
Write-Host "PASS: Working tree clean" -ForegroundColor Green

Write-Host ""
Write-Host "[2/8] Recent commits" -ForegroundColor Yellow
git log --oneline --decorate -8
if ($LASTEXITCODE -ne 0) { throw "git log failed" }

Write-Host ""
Write-Host "[3/8] Commit author check" -ForegroundColor Yellow
git log -1 --format="%H%n%an <%ae>%n%cn <%ce>%n%s"
if ($LASTEXITCODE -ne 0) { throw "git author check failed" }

Invoke-CmdStep "[4/8] cargo fmt -- --check" "cargo fmt -- --check" ".verify-fmt.txt" | Out-Null
Invoke-CmdStep "[5/8] cargo check" "cargo check" ".verify-check.txt" | Out-Null
Invoke-CmdStep "[6/8] cargo test --workspace" "cargo test --workspace" ".verify-test.txt" | Out-Null

Write-Host ""
Write-Host "[7/8] smoke-check script" -ForegroundColor Yellow
powershell -ExecutionPolicy Bypass -File ".\scripts\smoke-check.ps1"
if ($LASTEXITCODE -ne 0) {
    throw "smoke-check.ps1 failed"
}
Write-Host "PASS: smoke-check passed" -ForegroundColor Green

Write-Host ""
Write-Host "[8/8] Optional Ollama live check" -ForegroundColor Yellow

$ollamaCmd = Get-Command ollama -ErrorAction SilentlyContinue

if (-not $ollamaCmd) {
    Write-Host "SKIP: Ollama command not found. This is okay for normal tests." -ForegroundColor Yellow
} else {
    Write-Host "Ollama found." -ForegroundColor Cyan

    cmd /c "ollama list"
    if ($LASTEXITCODE -ne 0) {
        Write-Host "WARN: ollama list failed. Ollama may not be running." -ForegroundColor Yellow
    } else {
        $configPath = Join-Path $env:APPDATA "sc-agent\config.toml"

        if (-not (Test-Path $configPath)) {
            Write-Host "SKIP: sc-agent live Ollama commands need config first: $configPath" -ForegroundColor Yellow
            Write-Host "Run manually when desired: cargo run -- init" -ForegroundColor Yellow
        } else {
            Invoke-CmdStep "sc-agent models list" "cargo run -- models list" ".verify-models.txt" | Out-Null
            Invoke-CmdStep "sc-agent doctor" "cargo run -- doctor" ".verify-doctor.txt" | Out-Null
            Invoke-CmdStep "sc-agent short Ollama prompt" "cargo run -- run `"Say hello in one short sentence.`"" ".verify-run.txt" | Out-Null
        }
    }
}

Remove-Item ".verify-*.txt",".smoke-*.txt" -Force -ErrorAction SilentlyContinue

Write-Host ""
Write-Host "==================================================" -ForegroundColor Green
Write-Host " SC NODE VERIFY COMPLETE" -ForegroundColor Green
Write-Host "==================================================" -ForegroundColor Green
Write-Host ""

git status