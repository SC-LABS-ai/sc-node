param(
    [string]$BaseUrl = "https://openrouter.ai/api/v1",
    [string]$DefaultModel = "openai/gpt-4.1-mini",
    [string]$ResearchModel = "perplexity/sonar-pro"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    Write-Host "[openrouter] deterministic adapter tests"
    & cargo test --locked -p sc-provider-openrouter
    if ($LASTEXITCODE -ne 0) { throw "OpenRouter crate tests failed" }

    Write-Host "[openrouter] public catalog schema"
    $catalog = Invoke-RestMethod -Method Get -Uri ($BaseUrl.TrimEnd('/') + "/models") -TimeoutSec 30
    $models = @($catalog.data)
    if ($models.Count -eq 0) { throw "OpenRouter catalog returned no models" }

    $invalid = @($models | Where-Object {
        [string]::IsNullOrWhiteSpace($_.id) -or
        [string]::IsNullOrWhiteSpace($_.name) -or
        $null -eq $_.context_length -or
        [uint64]$_.context_length -eq 0 -or
        $null -eq $_.supported_parameters
    })
    if ($invalid.Count -ne 0) {
        throw "OpenRouter catalog has $($invalid.Count) entries missing required metadata"
    }

    $ids = @($models | ForEach-Object { $_.id })
    foreach ($required in @($DefaultModel, $ResearchModel)) {
        if ($ids -notcontains $required) {
            throw "Configured OpenRouter model '$required' is missing from the live catalog"
        }
    }

    $toolModels = @($models | Where-Object { @($_.supported_parameters) -contains "tools" }).Count
    Write-Host ("PUBLIC_CATALOG=PASS models={0} tool_capable={1}" -f $models.Count, $toolModels)

    if ([string]::IsNullOrWhiteSpace($env:SC_AGENT_OPENROUTER_API_KEY)) {
        Write-Host "AUTHENTICATED_COMPLETION=SKIP SC_AGENT_OPENROUTER_API_KEY is not set"
        Write-Host "OPENROUTER_GATE=PASS_WITH_AUTH_SKIP"
        exit 0
    }

    Write-Host "[openrouter] authenticated adapter catalog + streaming completion"
    & cargo test --locked -p sc-provider-openrouter tests::live_authenticated_catalog_and_completion -- --ignored --exact --nocapture
    if ($LASTEXITCODE -ne 0) { throw "Authenticated OpenRouter live test failed" }
    Write-Host "AUTHENTICATED_COMPLETION=PASS"
    Write-Host "OPENROUTER_GATE=PASS"
}
finally {
    Pop-Location
}
