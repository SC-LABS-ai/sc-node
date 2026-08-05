param(
    [string]$Model = "qwen:latest",
    [string]$Prompt = "Reply exactly with OK.",
    [string]$BaseUrl = "http://127.0.0.1:11434",
    [int]$WarmIterations = 8,
    [int]$ColdIterations = 3,
    [int]$ModelTimeoutSecs = 300,
    [int]$SyntheticIterations = 100,
    [int]$FixtureIterations = 20,
    [int]$MicroIterations = 100000,
    [int]$MicroIoIterations = 1000,
    [string]$OutputDirectory = ""
)

$ErrorActionPreference = "Stop"
$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
if (-not $OutputDirectory) {
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
    $OutputDirectory = Join-Path $repoRoot "artifacts\benchmarks\$stamp"
}
$OutputDirectory = [System.IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$lockPath = Join-Path $OutputDirectory ".benchmark.lock"
try {
    $lockStream = [System.IO.File]::Open(
        $lockPath,
        [System.IO.FileMode]::OpenOrCreate,
        [System.IO.FileAccess]::ReadWrite,
        [System.IO.FileShare]::None
    )
} catch {
    throw "Another benchmark process already owns '$OutputDirectory'. Use a different output directory or wait for it to finish."
}

$binaryName = if ($IsLinux -or $IsMacOS) { "sc-benchmark" } else { "sc-benchmark.exe" }
$binary = Join-Path $repoRoot "target\release\$binaryName"

function Quote-NativeArgument {
    param([string]$Value)
    if ($Value -notmatch '[\s"]') { return $Value }
    return '"' + ($Value -replace '(\\*)"', '$1$1\"' -replace '(\\+)$', '$1$1') + '"'
}

function Get-Percentile {
    param([double[]]$Values, [double]$Percentile)
    if (-not $Values -or $Values.Count -eq 0) { return $null }
    $sorted = @($Values | Sort-Object)
    $rank = [Math]::Ceiling(($Percentile / 100.0) * $sorted.Count) - 1
    $rank = [Math]::Max(0, [Math]::Min($rank, $sorted.Count - 1))
    return [double]$sorted[$rank]
}

function Get-Median {
    param([double[]]$Values)
    if (-not $Values -or $Values.Count -eq 0) { return $null }
    $sorted = @($Values | Sort-Object)
    $middle = [Math]::Floor($sorted.Count / 2)
    if ($sorted.Count % 2 -eq 1) { return [double]$sorted[$middle] }
    return ([double]$sorted[$middle - 1] + [double]$sorted[$middle]) / 2.0
}

function Invoke-MeasuredProcess {
    param(
        [Parameter(Mandatory = $true)][string[]]$Arguments,
        [Parameter(Mandatory = $true)][string]$Category,
        [Parameter(Mandatory = $true)][string]$Phase,
        [Parameter(Mandatory = $true)][int]$Iteration,
        [Parameter(Mandatory = $true)][string]$Mode
    )

    $argumentString = ($Arguments | ForEach-Object { Quote-NativeArgument $_ }) -join ' '
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $binary
    $psi.Arguments = $argumentString
    $psi.WorkingDirectory = $repoRoot
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true

    $process = New-Object System.Diagnostics.Process
    $process.StartInfo = $psi
    $startedAt = [DateTimeOffset]::UtcNow
    $stopwatch = [System.Diagnostics.Stopwatch]::StartNew()
    if (-not $process.Start()) { throw "Failed to start benchmark helper" }
    $stdoutTask = $process.StandardOutput.ReadToEndAsync()
    $stderrTask = $process.StandardError.ReadToEndAsync()
    $peakWorkingSet = 0L
    while (-not $process.HasExited) {
        try {
            $process.Refresh()
            if ($process.WorkingSet64 -gt $peakWorkingSet) {
                $peakWorkingSet = $process.WorkingSet64
            }
        } catch {}
        Start-Sleep -Milliseconds 10
    }
    $process.WaitForExit()
    $stopwatch.Stop()
    $stdout = $stdoutTask.GetAwaiter().GetResult()
    $stderr = $stderrTask.GetAwaiter().GetResult()
    try {
        $process.Refresh()
        if ($process.PeakWorkingSet64 -gt $peakWorkingSet) {
            $peakWorkingSet = $process.PeakWorkingSet64
        }
        $cpuMs = $process.TotalProcessorTime.TotalMilliseconds
    } catch {
        $cpuMs = $null
    }

    $safeName = "{0}-{1}-{2:D2}-{3}" -f $Category, $Phase, $Iteration, $Mode
    [System.IO.File]::WriteAllText((Join-Path $OutputDirectory "$safeName.stdout.txt"), $stdout)
    [System.IO.File]::WriteAllText((Join-Path $OutputDirectory "$safeName.stderr.txt"), $stderr)

    $structured = $null
    foreach ($line in ($stdout -split "`r?`n")) {
        if ($line.StartsWith("BENCH_RESULT=")) {
            $structured = $line.Substring("BENCH_RESULT=".Length) | ConvertFrom-Json
        }
        if ($line.StartsWith("SYNTHETIC_RESULT=")) {
            $structured = $line.Substring("SYNTHETIC_RESULT=".Length) | ConvertFrom-Json
        }
    }

    [PSCustomObject]@{
        category = $Category
        phase = $Phase
        iteration = $Iteration
        mode = $Mode
        started_utc = $startedAt.ToString("o")
        exit_code = $process.ExitCode
        wall_ms = [Math]::Round($stopwatch.Elapsed.TotalMilliseconds, 4)
        cpu_ms = if ($null -eq $cpuMs) { $null } else { [Math]::Round($cpuMs, 4) }
        peak_working_set_bytes = $peakWorkingSet
        internal_elapsed_ms = if ($structured -and $structured.elapsed_ms) { [double]$structured.elapsed_ms } else { $null }
        internal_ns_per_iteration = if ($structured -and $structured.ns_per_iteration) { [double]$structured.ns_per_iteration } else { $null }
        stdout_bytes = [Text.Encoding]::UTF8.GetByteCount($stdout)
        stderr_bytes = [Text.Encoding]::UTF8.GetByteCount($stderr)
    }
}

function Invoke-Unload {
    & $binary unload-ollama --model $Model --base-url $BaseUrl | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "Failed to unload Ollama model '$Model'" }
    Start-Sleep -Milliseconds 750
}

Push-Location $repoRoot
try {
    Write-Host "Building sc-benchmark from the committed lockfile..."
    & cargo build --release --locked -p sc-benchmark
    if ($LASTEXITCODE -ne 0) { throw "Benchmark helper build failed" }

    $gitSha = (& git rev-parse HEAD).Trim()
    $gitStatus = (& git status --porcelain) -join "`n"
    $rustc = (& rustc -vV) -join "`n"
    $cargoVersion = (& cargo --version).Trim()
    $runtime = [System.Runtime.InteropServices.RuntimeInformation]
    $environment = [ordered]@{
        generated_utc = [DateTimeOffset]::UtcNow.ToString("o")
        repository = "https://github.com/SC-LABS-ai/sc-node"
        commit = $gitSha
        dirty = [bool]$gitStatus
        dirty_status = $gitStatus
        model = $Model
        prompt = $Prompt
        base_url = $BaseUrl
        warm_iterations = $WarmIterations
        cold_iterations = $ColdIterations
        model_timeout_secs = $ModelTimeoutSecs
        synthetic_iterations = $SyntheticIterations
        fixture_iterations = $FixtureIterations
        micro_iterations = $MicroIterations
        micro_io_iterations = $MicroIoIterations
        os_description = $runtime::OSDescription
        os_architecture = $runtime::OSArchitecture.ToString()
        process_architecture = $runtime::ProcessArchitecture.ToString()
        processor_count = [Environment]::ProcessorCount
        machine_name = [Environment]::MachineName
        rustc = $rustc
        cargo = $cargoVersion
        powershell = $PSVersionTable.PSVersion.ToString()
        memory_scope = "Peak working set is measured for the benchmark helper/SC Node process only; the external Ollama server and model memory are excluded."
    }
    if (-not ($IsLinux -or $IsMacOS)) {
        try {
            $cpu = Get-CimInstance Win32_Processor | Select-Object -First 1
            $computer = Get-CimInstance Win32_ComputerSystem
            $environment.cpu_name = $cpu.Name.Trim()
            $environment.total_physical_memory_bytes = [long]$computer.TotalPhysicalMemory
        } catch {}
    }
    $environment | ConvertTo-Json -Depth 8 | Set-Content -Encoding UTF8 (Join-Path $OutputDirectory "environment.json")

    Write-Host "Running deterministic microbenchmarks..."
    $microOutput = & $binary micro --iterations $MicroIterations --io-iterations $MicroIoIterations
    if ($LASTEXITCODE -ne 0) { throw "Microbenchmark failed" }
    $microLines = @($microOutput | Where-Object { $_.StartsWith("MICRO_RESULT=") } | ForEach-Object { $_.Substring("MICRO_RESULT=".Length) })
    $microLines | Set-Content -Encoding UTF8 (Join-Path $OutputDirectory "micro.jsonl")

    $results = New-Object System.Collections.ArrayList
    Write-Host "Running synthetic no-model agent paths..."
    [void]$results.Add((Invoke-MeasuredProcess -Arguments @("synthetic-agent", "--iterations", "$SyntheticIterations") -Category "synthetic" -Phase "steady" -Iteration 1 -Mode "no_tool"))
    [void]$results.Add((Invoke-MeasuredProcess -Arguments @("synthetic-agent", "--iterations", "$SyntheticIterations", "--tool-round") -Category "synthetic" -Phase "steady" -Iteration 1 -Mode "tool_no_audit"))
    [void]$results.Add((Invoke-MeasuredProcess -Arguments @("synthetic-agent", "--iterations", "$SyntheticIterations", "--tool-round", "--audit") -Category "synthetic" -Phase "steady" -Iteration 1 -Mode "tool_with_audit"))


    Write-Host "Running $FixtureIterations paired iterations against a deterministic local Ollama-compatible fixture..."
    $portProbe = New-Object System.Net.Sockets.TcpListener([System.Net.IPAddress]::Loopback, 0)
    $portProbe.Start()
    $fixturePort = ([System.Net.IPEndPoint]$portProbe.LocalEndpoint).Port
    $portProbe.Stop()
    $fixtureUrl = "http://127.0.0.1:$fixturePort"
    $mockStdout = Join-Path $OutputDirectory "fixture-server.stdout.txt"
    $mockStderr = Join-Path $OutputDirectory "fixture-server.stderr.txt"
    $mockProcess = Start-Process -FilePath $binary -ArgumentList @("serve-mock-ollama", "--bind", "127.0.0.1:$fixturePort") -RedirectStandardOutput $mockStdout -RedirectStandardError $mockStderr -PassThru -NoNewWindow
    try {
        $fixtureReady = $false
        for ($attempt = 0; $attempt -lt 100; $attempt++) {
            if ($mockProcess.HasExited) {
                throw "Fixture server exited before becoming ready"
            }
            try {
                $tags = Invoke-RestMethod -Uri "$fixtureUrl/api/tags" -TimeoutSec 1
                if ($tags.models[0].name -eq "fixture") {
                    $fixtureReady = $true
                    break
                }
            } catch {}
            Start-Sleep -Milliseconds 50
        }
        if (-not $fixtureReady) { throw "Fixture server did not become ready" }

        for ($i = 1; $i -le $FixtureIterations; $i++) {
            $order = if ($i % 2 -eq 1) { @("direct", "node") } else { @("node", "direct") }
            foreach ($mode in $order) {
                $command = if ($mode -eq "direct") { "direct-ollama" } else { "node-ollama" }
                $row = Invoke-MeasuredProcess -Arguments @($command, "--model", "fixture", "--prompt", $Prompt, "--base-url", $fixtureUrl) -Category "fixture" -Phase "instant" -Iteration $i -Mode $mode
                if ($row.exit_code -ne 0) { throw "$mode fixture iteration $i failed" }
                [void]$results.Add($row)
            }
        }
    }
    finally {
        if ($mockProcess -and -not $mockProcess.HasExited) {
            $mockProcess.Kill()
            $mockProcess.WaitForExit()
        }
    }

    Write-Host "Pre-warming the selected model..."
    & $binary direct-ollama --model $Model --prompt $Prompt --base-url $BaseUrl --timeout-secs $ModelTimeoutSecs | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "Ollama warm-up failed" }

    Write-Host "Running $WarmIterations warm paired iterations with alternating order..."
    for ($i = 1; $i -le $WarmIterations; $i++) {
        $order = if ($i % 2 -eq 1) { @("direct", "node") } else { @("node", "direct") }
        foreach ($mode in $order) {
            $command = if ($mode -eq "direct") { "direct-ollama" } else { "node-ollama" }
            $row = Invoke-MeasuredProcess -Arguments @($command, "--model", $Model, "--prompt", $Prompt, "--base-url", $BaseUrl, "--timeout-secs", "$ModelTimeoutSecs") -Category "provider" -Phase "warm" -Iteration $i -Mode $mode
            if ($row.exit_code -ne 0) { throw "$mode warm iteration $i failed" }
            [void]$results.Add($row)
        }
    }

    Write-Host "Running $ColdIterations cold-model iterations per path..."
    for ($i = 1; $i -le $ColdIterations; $i++) {
        foreach ($mode in @("direct", "node")) {
            Invoke-Unload
            $command = if ($mode -eq "direct") { "direct-ollama" } else { "node-ollama" }
            $row = Invoke-MeasuredProcess -Arguments @($command, "--model", $Model, "--prompt", $Prompt, "--base-url", $BaseUrl, "--timeout-secs", "$ModelTimeoutSecs") -Category "provider" -Phase "cold" -Iteration $i -Mode $mode
            if ($row.exit_code -ne 0) { throw "$mode cold iteration $i failed" }
            [void]$results.Add($row)
        }
    }

    $results | Export-Csv -NoTypeInformation -Encoding UTF8 (Join-Path $OutputDirectory "raw-process-measurements.csv")

    function Add-SummaryRows {
        param(
            [System.Collections.ArrayList]$Target,
            [object[]]$Source,
            [string[]]$Phases
        )
        foreach ($phase in $Phases) {
            foreach ($mode in @("direct", "node")) {
                $rows = @($Source | Where-Object { $_.phase -eq $phase -and $_.mode -eq $mode })
                $walls = [double[]]@($rows | ForEach-Object { $_.wall_ms })
                $cpus = [double[]]@($rows | Where-Object { $null -ne $_.cpu_ms } | ForEach-Object { $_.cpu_ms })
                $peaks = [double[]]@($rows | ForEach-Object { $_.peak_working_set_bytes })
                [void]$Target.Add([PSCustomObject]@{
                    phase = $phase
                    mode = $mode
                    count = $rows.Count
                    median_wall_ms = [Math]::Round((Get-Median $walls), 4)
                    p95_wall_ms = [Math]::Round((Get-Percentile $walls 95), 4)
                    median_cpu_ms = if ($cpus.Count) { [Math]::Round((Get-Median $cpus), 4) } else { $null }
                    median_peak_working_set_bytes = [long](Get-Median $peaks)
                })
            }
        }
    }

    $provider = @($results | Where-Object { $_.category -eq "provider" })
    $fixture = @($results | Where-Object { $_.category -eq "fixture" })
    $summaryRows = New-Object System.Collections.ArrayList
    $fixtureRows = New-Object System.Collections.ArrayList
    Add-SummaryRows -Target $summaryRows -Source $provider -Phases @("warm", "cold")
    Add-SummaryRows -Target $fixtureRows -Source $fixture -Phases @("instant")

    $warmDirect = ($summaryRows | Where-Object { $_.phase -eq "warm" -and $_.mode -eq "direct" }).median_wall_ms
    $warmNode = ($summaryRows | Where-Object { $_.phase -eq "warm" -and $_.mode -eq "node" }).median_wall_ms
    $coldDirect = ($summaryRows | Where-Object { $_.phase -eq "cold" -and $_.mode -eq "direct" }).median_wall_ms
    $coldNode = ($summaryRows | Where-Object { $_.phase -eq "cold" -and $_.mode -eq "node" }).median_wall_ms
    $fixtureDirect = ($fixtureRows | Where-Object { $_.mode -eq "direct" }).median_wall_ms
    $fixtureNode = ($fixtureRows | Where-Object { $_.mode -eq "node" }).median_wall_ms
    $comparison = [ordered]@{
        fixture_overhead_ms = [Math]::Round($fixtureNode - $fixtureDirect, 4)
        fixture_overhead_percent = if ($fixtureDirect -ne 0) { [Math]::Round((($fixtureNode - $fixtureDirect) / $fixtureDirect) * 100.0, 4) } else { $null }
        real_model_warm_observed_difference_ms = [Math]::Round($warmNode - $warmDirect, 4)
        real_model_warm_observed_difference_percent = if ($warmDirect -ne 0) { [Math]::Round((($warmNode - $warmDirect) / $warmDirect) * 100.0, 4) } else { $null }
        real_model_cold_observed_difference_ms = [Math]::Round($coldNode - $coldDirect, 4)
        real_model_cold_observed_difference_percent = if ($coldDirect -ne 0) { [Math]::Round((($coldNode - $coldDirect) / $coldDirect) * 100.0, 4) } else { $null }
    }
    $synthetic = @($results | Where-Object { $_.category -eq "synthetic" } | Select-Object mode, wall_ms, cpu_ms, peak_working_set_bytes, internal_ns_per_iteration)
    $summary = [ordered]@{
        environment = $environment
        fixture_summary = @($fixtureRows)
        provider_summary = @($summaryRows)
        comparison = $comparison
        synthetic_summary = $synthetic
        caveats = @(
            "Fixture measurements use a deterministic local Ollama-compatible HTTP endpoint and include process startup and output formatting for both paths.",
            "Real-model provider measurements include model/server variability and are observational differences, not a pure runtime overhead estimate.",
            "Cold runs deliberately unload the Ollama model before each measured process and are dominated by model loading.",
            "Warm run order alternates to reduce order bias.",
            "Peak working set excludes the external Ollama server and model memory.",
            "This is one machine and one model, not a universal performance claim."
        )
    }
    $summary | ConvertTo-Json -Depth 12 | Set-Content -Encoding UTF8 (Join-Path $OutputDirectory "summary.json")

    $report = @"
# SC Node overhead benchmark

Generated: $($environment.generated_utc)  
Commit: ``$gitSha``  
Model: ``$Model``  
OS: $($environment.os_description)  
CPU: $($environment.cpu_name)  

## Deterministic fixture endpoint

| Path | N | Median wall | p95 wall | Median CPU | Median process peak RAM |
|---|---:|---:|---:|---:|---:|
$((@($fixtureRows) | ForEach-Object { "| $($_.mode) | $($_.count) | $($_.median_wall_ms) ms | $($_.p95_wall_ms) ms | $($_.median_cpu_ms) ms | $([Math]::Round($_.median_peak_working_set_bytes / 1MB, 2)) MiB |" }) -join "`n")

Fixture median overhead (SC Node minus direct): **$($comparison.fixture_overhead_ms) ms** (**$($comparison.fixture_overhead_percent)%**).

The fixture is a local Ollama-compatible HTTP server returning a fixed immediate response. It isolates process startup, routing, session construction, provider-adapter work, parsing, and output formatting from model inference.

## Real Ollama model observations

| Phase | Path | N | Median wall | p95 wall | Median CPU | Median process peak RAM |
|---|---:|---:|---:|---:|---:|---:|
$((@($summaryRows) | ForEach-Object { "| $($_.phase) | $($_.mode) | $($_.count) | $($_.median_wall_ms) ms | $($_.p95_wall_ms) ms | $($_.median_cpu_ms) ms | $([Math]::Round($_.median_peak_working_set_bytes / 1MB, 2)) MiB |" }) -join "`n")

Warm observed median difference (SC Node minus direct): **$($comparison.real_model_warm_observed_difference_ms) ms** (**$($comparison.real_model_warm_observed_difference_percent)%**).  
Cold observed median difference: **$($comparison.real_model_cold_observed_difference_ms) ms** (**$($comparison.real_model_cold_observed_difference_percent)%**).

## Interpretation

The direct and SC Node paths use the same Ollama endpoint, model, prompt, streaming request, temperature, token limit, and keep-alive setting. Warm order alternates. Cold runs unload the model before every measurement. Real-model differences include inference and model-server variance and therefore are not treated as pure framework overhead. These figures describe this exact machine and model only; they are not a universal Rust-vs-Python or SC-Node-vs-framework claim.

Process peak RAM covers only the measured helper/SC Node process. Ollama and model memory are external and excluded.

## Raw evidence

- ``environment.json``
- ``raw-process-measurements.csv``
- ``micro.jsonl``
- per-run stdout/stderr files
- ``summary.json``
"@
    [System.IO.File]::WriteAllText((Join-Path $OutputDirectory "REPORT.md"), $report, [System.Text.UTF8Encoding]::new($false))

    Write-Host ""
    Write-Host "Benchmark complete: $OutputDirectory" -ForegroundColor Green
    Write-Host ("Fixture median overhead: {0} ms ({1}%)" -f $comparison.fixture_overhead_ms, $comparison.fixture_overhead_percent)
    Write-Host ("Real-model warm observed difference: {0} ms ({1}%)" -f $comparison.real_model_warm_observed_difference_ms, $comparison.real_model_warm_observed_difference_percent)
}
finally {
    Pop-Location
    if ($lockStream) {
        $lockStream.Dispose()
    }
}
