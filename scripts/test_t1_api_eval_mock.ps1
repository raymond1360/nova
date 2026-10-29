#requires -Version 7.0
param(
 [Parameter(Mandatory=$true)][string]$NodePath,
 [Parameter(Mandatory=$true)][string]$VerifierDeps,
 [Parameter(Mandatory=$true)][string]$EvidenceDirectory
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (-not [IO.Path]::IsPathRooted($NodePath) -or
    (Get-FileHash -LiteralPath $NodePath -Algorithm SHA256).Hash.ToLowerInvariant() -cne '63c259c81e5d472b5f11c8d506070130cb04a1ecf84b80377a34ed6ec9048088') { throw 'Pinned absolute official Windows x64 Node required' }
if (Test-Path -LiteralPath $EvidenceDirectory) { throw 'New evidence directory required' }
New-Item -ItemType Directory -Path $EvidenceDirectory | Out-Null
$nodeHome=Join-Path $EvidenceDirectory 'node-owned-home';New-Item -ItemType Directory -Path $nodeHome | Out-Null
function Invoke-NodeClean([string[]]$Vector,[string]$Name) {
 $info=[Diagnostics.ProcessStartInfo]::new();$info.FileName=$NodePath;$info.UseShellExecute=$false;$info.CreateNoWindow=$true
 $info.RedirectStandardOutput=$true;$info.RedirectStandardError=$true;$info.Environment.Clear()
 $system=[Environment]::GetFolderPath([Environment+SpecialFolder]::Windows)
 $info.Environment['SystemRoot']=$system;$info.Environment['PATH']=(Split-Path $NodePath)
 foreach($key in @('TEMP','TMP','HOME','USERPROFILE','APPDATA','LOCALAPPDATA')){$info.Environment[$key]=$nodeHome}
 foreach($arg in $Vector){$info.ArgumentList.Add($arg)}
 $p=[Diagnostics.Process]::new();$p.StartInfo=$info;if(-not $p.Start()){throw 'Node verifier start failed'}
 $out=$p.StandardOutput.ReadToEndAsync();$err=$p.StandardError.ReadToEndAsync()
 if(-not $p.WaitForExit(30000)){$p.Kill($true);$p.WaitForExit();throw 'Node verifier deadline'}
 [IO.File]::WriteAllText((Join-Path $EvidenceDirectory ($Name+'.stdout.txt')),$out.GetAwaiter().GetResult())
 [IO.File]::WriteAllText((Join-Path $EvidenceDirectory ($Name+'.stderr.txt')),$err.GetAwaiter().GetResult())
 $exit=$p.ExitCode;$p.Dispose();if($exit -ne 0){throw "Node verifier failed: $exit"}
}
Invoke-NodeClean @('--version') 'node-version'
if(([IO.File]::ReadAllText((Join-Path $EvidenceDirectory 'node-version.stdout.txt'))).Trim() -cne 'v24.14.0'){throw 'Node version mismatch'}
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
Invoke-NodeClean @((Join-Path $root 'tools/t1-api-eval/check-logo.cjs'),(Join-Path $VerifierDeps 'package.json'),'--self-test') 'logo-tests'
$global:LASTEXITCODE = 0
