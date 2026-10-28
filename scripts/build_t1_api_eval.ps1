#requires -Version 7.0
param([Parameter(Mandatory=$true)][string]$EvidenceDirectory)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (Test-Path -LiteralPath $EvidenceDirectory) { throw 'New evidence directory required' }
New-Item -ItemType Directory -Path $EvidenceDirectory | Out-Null
$env:CARGO_TARGET_DIR = Join-Path $EvidenceDirectory 'cargo-target'
Push-Location (Join-Path $root 'apps/desktop/src-tauri')
try {
    & cargo build --offline --locked --features t1-api-eval --bin nova_t1_api_eval 2>&1 | Tee-Object (Join-Path $EvidenceDirectory 'debug-build.log')
    if ($LASTEXITCODE -ne 0) { throw 'Debug build failed' }
} finally { Pop-Location }
# Build only. No helper, Agent, probe, app or target execution.
