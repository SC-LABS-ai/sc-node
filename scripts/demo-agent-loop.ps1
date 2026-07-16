#!/usr/bin/env powershell

# SC Node - Deterministic Agent Loop Demo
# This script proves the tool-result feedback loop works without relying on Ollama

$ErrorActionPreference = "Stop"

Write-Host ""
Write-Host "==================================================" -ForegroundColor Cyan
Write-Host " SC NODE - DETERMINISTIC AGENT LOOP DEMO" -ForegroundColor Cyan
Write-Host "==================================================" -ForegroundColor Cyan
Write-Host ""

Write-Host "[1/3] Building project..." -ForegroundColor Yellow
cargo build 2>&1 | Select-Object -Last 3
if ($LASTEXITCODE -ne 0) {
    Write-Host "FAIL: Build failed" -ForegroundColor Red
    exit 1
}
Write-Host "PASS: Build succeeded" -ForegroundColor Green

Write-Host ""
Write-Host "[2/3] Running deterministic agent loop test..." -ForegroundColor Yellow

# We'll use the fake provider test via cargo test
$testResult = cargo test --test agent_loop_tests 2>&1
Write-Host $testResult
if ($LASTEXITCODE -ne 0) {
    Write-Host "FAIL: Agent loop tests failed" -ForegroundColor Red
    exit 1
}
Write-Host "PASS: All agent loop tests passed" -ForegroundColor Green

Write-Host ""
Write-Host "[3/3] Running live demo with Ollama (if available)..." -ForegroundColor Yellow
$ollamaCheck = Get-Command ollama -ErrorAction SilentlyContinue
if (-not $ollamaCheck) {
    Write-Host "SKIP: Ollama not installed, skipping live demo" -ForegroundColor Yellow
} else {
    $ollamaRunning = ollama list 2>&1
    if ($LASTEXITCODE -eq 0) {
        Write-Host "Ollama running, running live demo..." -ForegroundColor Cyan
        cargo run -- run "Use the list_dir tool with path '.' to list the project root, then read README.md and tell me the project name" 2>&1 | Select-Object -Last 10
    } else {
        Write-Host "SKIP: Ollama not running, skipping live demo" -ForegroundColor Yellow
    }
}

Write-Host ""
Write-Host "==================================================" -ForegroundColor Green
Write-Host " DEMO COMPLETE - AGENT LOOP VERIFIED" -ForegroundColor Green
Write-Host "==================================================" -ForegroundColor Green
Write-Host ""

Write-Host "Summary:"
Write-Host "  [OK] Unit tests pass (agent loop logic verified)"
Write-Host "  [OK] Max tool rounds enforced (3 rounds max)"
Write-Host "  [OK] Tool errors captured in history"
Write-Host "  [OK] Unknown tools handled gracefully"
Write-Host "  [OK] Tool results fed back to model in next round"
Write-Host ""

Write-Host "The agent loop is proven to work deterministically." -ForegroundColor Green
Write-Host ""

Write-Host "Remaining limitations:"
Write-Host "  - Real model tool-calling reliability varies by model"
Write-Host "  - Ollama tool calling format varies by model"
Write-Host "  - Routing rules not yet implemented"
Write-Host "  - No incremental streaming (batch collect then parse)"
Write-Host ""