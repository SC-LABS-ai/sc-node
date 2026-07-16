# SC Node - Public Beta Verification
#
# Purpose: run the checks a "public beta" release claim would need to stand
# behind, and report each one as PASS / FAIL / SKIP. The one rule that
# matters more than any individual check: this script must NEVER print PASS
# for a gate that is not actually wired up. Anything not implemented yet is
# reported SKIP with a short reason -- never PASS, and never silently
# omitted.
#
# Safe by design:
#   - Only runs local cargo commands, a read-only text scan of tracked
#     files, and (optionally) a single lightweight HTTP GET against a
#     locally-running Ollama or an NVIDIA NIM endpoint if credentials are
#     already present in the environment.
#   - Never prints, logs, or otherwise reveals API key values.
#   - Never deletes, moves, or overwrites project files (only removes its
#     own temporary output files, which are prefixed with ".verify-pb-").
#
# Exit code: 0 unless at least one gate FAILs (SKIP does not fail the run).

$ErrorActionPreference = "Stop"

Push-Location (Join-Path $PSScriptRoot "..")

$results = New-Object System.Collections.ArrayList

function Add-Result {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][ValidateSet("PASS", "FAIL", "SKIP")][string]$Status,
        [string]$Detail = ""
    )

    $entry = [PSCustomObject]@{
        Name   = $Name
        Status = $Status
        Detail = $Detail
    }
    [void]$results.Add($entry)

    $color = "White"
    if ($Status -eq "PASS") { $color = "Green" }
    if ($Status -eq "FAIL") { $color = "Red" }
    if ($Status -eq "SKIP") { $color = "Yellow" }

    Write-Host ("[{0}] {1}" -f $Status, $Name) -ForegroundColor $color
    if ($Detail) {
        $Detail -split "`r?`n" | ForEach-Object { Write-Host ("       " + $_) -ForegroundColor DarkGray }
    }
}

function Invoke-Gate {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Command,
        [Parameter(Mandatory = $true)][string]$OutputFile
    )

    Remove-Item $OutputFile -Force -ErrorAction SilentlyContinue

    cmd /c "$Command > `"$OutputFile`" 2>&1"
    $exitCode = $LASTEXITCODE

    $output = ""
    if (Test-Path $OutputFile) { $output = Get-Content $OutputFile -Raw }
    Remove-Item $OutputFile -Force -ErrorAction SilentlyContinue

    if ($exitCode -ne 0) {
        $tail = ""
        if ($output) {
            $tail = (($output -split "`r?`n") | Select-Object -Last 20) -join "`n"
        }
        Add-Result -Name $Name -Status "FAIL" -Detail ("exit code $exitCode`n$tail")
    } else {
        Add-Result -Name $Name -Status "PASS"
    }
    return $output
}

try {
    Write-Host ""
    Write-Host "==================================================" -ForegroundColor Cyan
    Write-Host " SC NODE - PUBLIC BETA VERIFICATION" -ForegroundColor Cyan
    Write-Host "==================================================" -ForegroundColor Cyan
    Write-Host "Rule: unimplemented gates report SKIP, never PASS." -ForegroundColor Cyan
    Write-Host ""

    # ── Build / lint / test gates ──────────────────────────────────────
    Invoke-Gate -Name "cargo fmt -- --check" -Command "cargo fmt -- --check" -OutputFile ".verify-pb-fmt.txt" | Out-Null
    Invoke-Gate -Name "cargo check --workspace" -Command "cargo check --workspace" -OutputFile ".verify-pb-check.txt" | Out-Null
    Invoke-Gate -Name "cargo clippy --workspace --all-targets -- -D warnings" -Command "cargo clippy --workspace --all-targets -- -D warnings" -OutputFile ".verify-pb-clippy.txt" | Out-Null
    Invoke-Gate -Name "cargo test --workspace" -Command "cargo test --workspace" -OutputFile ".verify-pb-test.txt" | Out-Null

    # ── Smoke check (delegates to the existing script, unmodified) ─────
    $smokeScript = Join-Path (Get-Location) "scripts\smoke-check.ps1"
    if (Test-Path $smokeScript) {
        cmd /c "powershell -NoProfile -ExecutionPolicy Bypass -File `"$smokeScript`" > `".verify-pb-smoke.txt`" 2>&1"
        $smokeExit = $LASTEXITCODE
        $smokeOut = ""
        if (Test-Path ".verify-pb-smoke.txt") { $smokeOut = Get-Content ".verify-pb-smoke.txt" -Raw }
        Remove-Item ".verify-pb-smoke.txt" -Force -ErrorAction SilentlyContinue

        if ($smokeExit -ne 0) {
            $tail = (($smokeOut -split "`r?`n") | Select-Object -Last 20) -join "`n"
            Add-Result -Name "smoke-check.ps1" -Status "FAIL" -Detail ("exit code $smokeExit`n$tail")
        } else {
            Add-Result -Name "smoke-check.ps1" -Status "PASS"
        }
    } else {
        Add-Result -Name "smoke-check.ps1" -Status "SKIP" -Detail "scripts\smoke-check.ps1 not found"
    }

    # ── Public-clean scan ───────────────────────────────────────────────
    # Scans every git-tracked file (this script and the local pattern file
    # below excluded, since they must contain the pattern list as literal
    # data) for internal codenames, private path fragments, and cloud API
    # key prefixes. This is a plain-text substring scan, not a
    # secret-detection engine -- it catches accidental leftovers, it does
    # not guarantee a clean repo.
    #
    # Generic, non-sensitive built-in patterns only. Anything specific to
    # this org (internal codenames, contact addresses, etc.) must NOT be
    # hardcoded here -- it is loaded from an optional untracked local file
    # instead (see below), so this script stays safe to publish as-is.
    # Content patterns. ".env" is intentionally NOT a content pattern: the
    # sandbox/contract deny-lists and their tests legitimately contain that
    # literal as protective data. Committed .env FILES are caught by the
    # tracked-filename check in the scan loop below instead.
    $genericPatterns = @(
        "C:\Users\",
        "BEGIN PRIVATE KEY",
        "BEGIN RSA PRIVATE KEY",
        "BEGIN OPENSSH PRIVATE KEY"
    )

    # Exact private patterns (internal codenames / contact / private path
    # fragments) are optional and untracked -- never committed to a public
    # repo. If the file is absent, this part of the scan is honestly
    # reported SKIP rather than silently omitted or falsely PASSed.
    $privatePatternsFile = Join-Path (Get-Location) "scripts\private-patterns.local.txt"
    $privatePatterns = @()
    $havePrivatePatterns = $false
    if (Test-Path $privatePatternsFile -PathType Leaf) {
        $privatePatterns = Get-Content $privatePatternsFile |
            ForEach-Object { $_.Trim() } |
            Where-Object { $_ -and -not $_.StartsWith("#") }
        if ($privatePatterns.Count -gt 0) { $havePrivatePatterns = $true }
    }

    $exactPatterns = @($genericPatterns) + @($privatePatterns)

    # Real-secret SHAPES (regex), not bare prefixes. A defensive denylist may
    # legitimately contain the literal prefix "nvapi-"/"sk-or-" as data (e.g.
    # the proof-bundle value scrubber), so we only flag a prefix that is
    # followed by a realistic key body -- an actual leaked credential -- and
    # a non-empty inline api_key assignment.
    $secretRegexes = @(
        'nvapi-[A-Za-z0-9_-]{16,}',
        'sk-or-v1-[A-Za-z0-9_-]{16,}',
        'api_key\s*=\s*"[A-Za-z0-9_\-]{20,}"'
    )

    $selfPath = $null
    if ($PSCommandPath) { $selfPath = (Resolve-Path $PSCommandPath).Path }

    $privatePatternsFullPath = $null
    if (Test-Path $privatePatternsFile -PathType Leaf) {
        $privatePatternsFullPath = (Resolve-Path $privatePatternsFile).Path
    }

    $trackedFiles = @()
    try {
        $trackedFiles = git ls-files 2>$null
    } catch {
        $trackedFiles = @()
    }

    $hits = New-Object System.Collections.ArrayList
    $scannedCount = 0
    foreach ($rel in $trackedFiles) {
        if (-not $rel) { continue }
        $full = Join-Path (Get-Location) $rel
        if (-not (Test-Path $full -PathType Leaf)) { continue }
        $fullResolved = (Resolve-Path $full).Path
        if ($selfPath -and $fullResolved -eq $selfPath) { continue }
        if ($privatePatternsFullPath -and $fullResolved -eq $privatePatternsFullPath) { continue }

        $scannedCount++

        # A tracked env/credential FILE must never ship, regardless of content.
        $leafName = Split-Path $rel -Leaf
        if ($leafName -like ".env*" -or $leafName -like "*.pem" -or
            $leafName -like "*.pfx" -or $leafName -like "id_rsa*") {
            [void]$hits.Add("${rel}: tracked credential-style filename")
        }

        try {
            $exactHits = Select-String -Path $full -Pattern $exactPatterns -SimpleMatch -ErrorAction SilentlyContinue
            $regexHits = Select-String -Path $full -Pattern $secretRegexes -ErrorAction SilentlyContinue
        } catch {
            $exactHits = $null; $regexHits = $null
        }
        foreach ($m in @($exactHits) + @($regexHits)) {
            if ($m) { [void]$hits.Add("${rel}:$($m.LineNumber): $($m.Line.Trim())") }
        }
    }

    if ($hits.Count -gt 0) {
        $detail = ($hits | Select-Object -First 20) -join "`n"
        Add-Result -Name "public-clean scan" -Status "FAIL" -Detail $detail
    } elseif (-not $havePrivatePatterns) {
        Add-Result -Name "public-clean scan" -Status "SKIP" -Detail "private pattern list not present (scripts/private-patterns.local.txt) -- generic built-in patterns only, $scannedCount tracked file(s) scanned"
    } else {
        Add-Result -Name "public-clean scan" -Status "PASS" -Detail "$scannedCount tracked file(s) scanned, 0 matches"
    }

    # ── Feature proofs (run the real crate tests when the crate is present) ─
    # Each proof runs the deterministic test suite for the crate that owns
    # the feature. If the crate is absent the gate degrades to SKIP -- it is
    # never reported PASS for something that is not there.
    function Invoke-CrateGate {
        param([string]$Name, [string]$Crate, [string]$Detail = "", [string]$Filter = "")
        if (Test-Path (Join-Path (Get-Location) "crates/$Crate/Cargo.toml")) {
            $cmd = "cargo test -p $Crate"
            if ($Filter) { $cmd = "$cmd $Filter" }
            Invoke-Gate -Name $Name -Command $cmd -OutputFile ".verify-pb-$Crate.txt" | Out-Null
        } else {
            Add-Result -Name $Name -Status "SKIP" -Detail "crate $Crate not present"
        }
    }

    # Permission gate: deterministic proofs that Deny never executes, Ask fails
    # closed non-interactively, unknown/malformed fail closed (atomic-counter
    # tests in sc-agent-core), plus the allow/deny pattern logic in sc-tool-core.
    Invoke-CrateGate -Name "permission gate proof (deny/ask fail-closed)" -Crate "sc-agent-core"
    Invoke-CrateGate -Name "allow/deny pattern proof" -Crate "sc-tool-core"
    Invoke-CrateGate -Name "Windows boundary proof" -Crate "sc-sandbox"
    Invoke-CrateGate -Name "routing proof (unit-level)" -Crate "sc-provider-core"
    Invoke-CrateGate -Name "contract proof" -Crate "sc-contract"
    Invoke-CrateGate -Name "proof-bundle verify" -Crate "sc-proof"
    Invoke-CrateGate -Name "local memory proof" -Crate "sc-memory"
    Invoke-CrateGate -Name "RAG fixture" -Crate "sc-memory" -Filter "rag"

    # Note: the *interactive* approval prompt for an Ask decision is deferred;
    # non-interactive fail-closed (the security-relevant direction) is proven
    # by the permission gate suite above.

    $turbovecFeature = $false
    $memCargoToml = Join-Path (Get-Location) "crates/sc-memory/Cargo.toml"
    if (Test-Path $memCargoToml) {
        $cargoTomlText = Get-Content $memCargoToml -Raw
        if ($cargoTomlText -match "(?im)^\s*turbovec\s*=") { $turbovecFeature = $true }
    }
    if ($turbovecFeature) {
        Invoke-Gate -Name "TurboVec (cargo test -p sc-memory --features turbovec)" -Command "cargo test -p sc-memory --features turbovec" -OutputFile ".verify-pb-turbovec.txt" | Out-Null
    } else {
        Add-Result -Name "TurboVec" -Status "SKIP" `
            -Detail "No optional 'turbovec' feature found in crates/sc-memory/Cargo.toml."
    }

    # ── Ollama health/live (best-effort, local-only, no secrets) ────────
    $ollamaLive = $false
    $ollamaDetail = "http://127.0.0.1:11434 not reachable (Ollama not running or not installed)"
    try {
        $resp = Invoke-WebRequest -Uri "http://127.0.0.1:11434/api/tags" -TimeoutSec 3 -UseBasicParsing -ErrorAction Stop
        if ($resp.StatusCode -eq 200) {
            $ollamaLive = $true
            $ollamaDetail = "GET http://127.0.0.1:11434/api/tags responded 200"
        }
    } catch {
        $ollamaLive = $false
    }
    if ($ollamaLive) {
        Add-Result -Name "Ollama health/live" -Status "PASS" -Detail $ollamaDetail
    } else {
        Add-Result -Name "Ollama health/live" -Status "SKIP" -Detail $ollamaDetail
    }

    # ── NVIDIA NIM live (only if credentials are present; key never printed) ─
    if ($env:SC_AGENT_NVIDIA_API_KEY) {
        try {
            $headers = @{ Authorization = "Bearer $($env:SC_AGENT_NVIDIA_API_KEY)" }
            $resp = Invoke-WebRequest -Uri "https://integrate.api.nvidia.com/v1/models" -Headers $headers -TimeoutSec 8 -UseBasicParsing -ErrorAction Stop
            if ($resp.StatusCode -eq 200) {
                Add-Result -Name "NVIDIA NIM live" -Status "PASS" -Detail "GET /v1/models responded 200"
            } else {
                Add-Result -Name "NVIDIA NIM live" -Status "FAIL" -Detail "GET /v1/models responded $($resp.StatusCode)"
            }
        } catch {
            Add-Result -Name "NVIDIA NIM live" -Status "FAIL" -Detail "Request failed (no key value logged): $($_.Exception.Message)"
        }
    } else {
        Add-Result -Name "NVIDIA NIM live" -Status "SKIP" -Detail "SC_AGENT_NVIDIA_API_KEY not set in environment"
    }
}
finally {
    Remove-Item ".verify-pb-*.txt" -Force -ErrorAction SilentlyContinue

    Write-Host ""
    Write-Host "==================================================" -ForegroundColor Cyan
    Write-Host " SUMMARY" -ForegroundColor Cyan
    Write-Host "==================================================" -ForegroundColor Cyan

    foreach ($r in $results) {
        $color = "White"
        if ($r.Status -eq "PASS") { $color = "Green" }
        if ($r.Status -eq "FAIL") { $color = "Red" }
        if ($r.Status -eq "SKIP") { $color = "Yellow" }
        Write-Host ("{0,-6} {1}" -f $r.Status, $r.Name) -ForegroundColor $color
    }

    # @(...) so a single match still counts as 1 (a bare scalar has no .Count
    # in Windows PowerShell and would silently report a failed gate as passed).
    $passCount = @($results | Where-Object { $_.Status -eq "PASS" }).Count
    $failCount = @($results | Where-Object { $_.Status -eq "FAIL" }).Count
    $skipCount = @($results | Where-Object { $_.Status -eq "SKIP" }).Count

    Write-Host ""
    Write-Host ("PASS: {0}   FAIL: {1}   SKIP: {2}" -f $passCount, $failCount, $skipCount) -ForegroundColor Cyan
    Write-Host ""

    if ($failCount -gt 0) {
        Write-Host "RESULT: FAIL ($failCount gate(s) failed)" -ForegroundColor Red
    } else {
        Write-Host "RESULT: NO FAILURES (SKIPs above are honest gaps, not passes)" -ForegroundColor Green
    }

    Pop-Location

    if ($failCount -gt 0) {
        exit 1
    }
    exit 0
}
