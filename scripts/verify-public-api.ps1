$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    function Invoke-Cargo {
        param([Parameter(Mandatory = $true)][string[]]$Arguments)
        & cargo @Arguments
        if ($LASTEXITCODE -ne 0) {
            throw "cargo $($Arguments -join ' ') failed with exit code $LASTEXITCODE"
        }
    }

    Write-Host "[public-api] external consumer"
    Invoke-Cargo @("test", "--locked", "-p", "sc-public-api-consumer")

    Write-Host "[public-api] rustdoc warnings are errors"
    $previousRustdocFlags = $env:RUSTDOCFLAGS
    try {
        $env:RUSTDOCFLAGS = "-D warnings"
        Invoke-Cargo @(
            "doc", "--locked", "--no-deps",
            "-p", "sc-message-types",
            "-p", "sc-provider-core",
            "-p", "sc-tool-core",
            "-p", "sc-contract",
            "-p", "sc-proof"
        )
    }
    finally {
        $env:RUSTDOCFLAGS = $previousRustdocFlags
    }

    Write-Host "[public-api] full Wave 1 package verification"
    foreach ($crate in @("sc-message-types", "sc-contract", "sc-proof")) {
        Invoke-Cargo @("package", "--locked", "--allow-dirty", "-p", $crate)
    }

    Write-Host "[public-api] later-wave package manifests and file sets"
    foreach ($crate in @("sc-provider-core", "sc-tool-core")) {
        Invoke-Cargo @("package", "--locked", "--allow-dirty", "--list", "-p", $crate)
    }

    Write-Host "PUBLIC_API_GATE=PASS"
}
finally {
    Pop-Location
}
