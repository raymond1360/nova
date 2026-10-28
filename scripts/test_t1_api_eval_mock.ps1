#requires -Version 7.0
param(
 [Parameter(Mandatory=$true)][string]$NodePath,
 [Parameter(Mandatory=$true)][string]$VerifierDeps,
 [Parameter(Mandatory=$true)][string]$EvidenceDirectory
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (-not [IO.Path]::IsPathRooted($NodePath) -or (& $NodePath --version) -cne 'v24.14.0' -or
    (Get-FileHash -LiteralPath $NodePath -Algorithm SHA256).Hash.ToLowerInvariant() -cne '63c259c81e5d472b5f11c8d506070130cb04a1ecf84b80377a34ed6ec9048088') { throw 'Pinned absolute official Windows x64 Node required' }
if (Test-Path -LiteralPath $EvidenceDirectory) { throw 'New evidence directory required' }
New-Item -ItemType Directory -Path $EvidenceDirectory | Out-Null
$env:CARGO_TARGET_DIR = Join-Path $EvidenceDirectory 'cargo-target'
Push-Location (Join-Path $root 'apps/desktop/src-tauri')
try {
 & cargo test --offline --locked --features t1-api-eval --lib t1_api_eval::tests -- --test-threads=1 2>&1 | Tee-Object (Join-Path $EvidenceDirectory 'mock-tests.log')
 if ($LASTEXITCODE -ne 0) { throw 'Mock tests failed' }
 & cargo build --offline --locked --features t1-api-eval --bin nova_t1_api_eval 2>&1 | Tee-Object (Join-Path $EvidenceDirectory 'build.log')
 if ($LASTEXITCODE -ne 0) { throw 'Debug build failed' }
 $savedNativePreference = $PSNativeCommandUseErrorActionPreference
 $PSNativeCommandUseErrorActionPreference = $false
 & cargo check --offline --locked --release --features t1-api-eval --lib --message-format=json 2>&1 | Tee-Object (Join-Path $EvidenceDirectory 'release-guard.log')
 $guardExit = $LASTEXITCODE
 $PSNativeCommandUseErrorActionPreference = $savedNativePreference
 $errors = @(Get-Content (Join-Path $EvidenceDirectory 'release-guard.log') | ForEach-Object { if ($_.StartsWith('{')) { $v = $_ | ConvertFrom-Json; if ($v.reason -eq 'compiler-message' -and $v.message.level -eq 'error') { $v } } })
 if ($guardExit -ne 101 -or $errors.Count -ne 1 -or $errors[0].target.name -cne 'nova_lib' -or $errors[0].message.message -cne 't1-api-eval is debug-only and must never ship') { throw 'Not the exact deliberate release guard' }
 & cargo check --offline --locked --lib --bins 2>&1 | Tee-Object (Join-Path $EvidenceDirectory 'default-check.log')
 if ($LASTEXITCODE -ne 0) { throw 'Default check failed' }
} finally { Pop-Location }
& $NodePath (Join-Path $root 'tools/t1-api-eval/check-logo.cjs') (Join-Path $VerifierDeps 'package.json') --self-test 2>&1 | Tee-Object (Join-Path $EvidenceDirectory 'logo-tests.log')
if ($LASTEXITCODE -ne 0) { throw 'Logo verification failed' }
$global:LASTEXITCODE = 0
