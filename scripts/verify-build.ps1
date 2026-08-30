<#
.SYNOPSIS
  Nova build/test verification script. Binds a build+test run to the exact
  Git commit under review and writes a human-readable summary to
  D:\nova\test-results\build-info.txt.

.DESCRIPTION
  Runs, in order: `npm test`, `npm run build`, `cargo test`, and
  `npm run tauri -- build` for apps/desktop, records each command's exit
  code, and reports the currently checked-out commit hash/branch/working
  tree status alongside the local toolchain versions.

  This script is read-only with respect to source: it never edits repo
  files, never touches .env/API keys, and never disables existing
  .gitignore rules for *.log/target/*.sqlite*.

.USAGE / WORKFLOW
  1. Finish and commit the code/doc changes you want verified.
  2. Run this script from a shell that can see the repo (any cwd is fine,
     paths below are absolute):
       pwsh -File D:\nova\scripts\verify-build.ps1
     or from Windows PowerShell:
       powershell -File D:\nova\scripts\verify-build.ps1
  3. Inspect D:\nova\test-results\build-info.txt. The "commit" field is the
     commit that was actually built/tested by this run.
  4. build-info.txt (and the verify-build-*.log files next to it) are
     regenerable verification artifacts, not source. Do not `git add` them
     as part of a source commit — regenerating this file after a commit
     must not itself require a new commit just to "update the hash".
     test-results/ is excluded from version control for this reason.

.EXIT CODE
  0   all steps passed.
  1   at least one step failed, or a required tool could not be located.
     The script never reports success while a step failed.
#>

[CmdletBinding()]
param()

$ErrorActionPreference = 'Continue'

$RepoRoot    = 'D:\nova'
$DesktopDir  = Join-Path $RepoRoot 'apps\desktop'
$TauriDir    = Join-Path $DesktopDir 'src-tauri'
$ResultsDir  = Join-Path $RepoRoot 'test-results'
$BuildInfoPath = Join-Path $ResultsDir 'build-info.txt'

if (-not (Test-Path $RepoRoot)) {
    Write-Error "Repo root not found: $RepoRoot"
    exit 1
}

if (-not (Test-Path $ResultsDir)) {
    New-Item -ItemType Directory -Path $ResultsDir | Out-Null
}

# ---------------------------------------------------------------------------
# Secret redaction (defense in depth). Nothing here is expected to contain
# secrets — these are build/test tool logs, not request logs — but scrub
# well-known sensitive patterns before anything is written to disk.
# ---------------------------------------------------------------------------
function Protect-Secrets {
    param([string]$Text)
    if ([string]::IsNullOrEmpty($Text)) { return $Text }
    $out = $Text
    $out = $out -replace '(?i)Authorization:\s*Bearer\s+\S+', 'Authorization: Bearer [REDACTED]'
    $out = $out -replace '(?i)Cookie:\s*\S+', 'Cookie: [REDACTED]'
    $out = $out -replace 'sk-[A-Za-z0-9_-]{8,}', '[REDACTED_KEY]'
    $out = $out -replace '(?im)^(.*(API[_-]?KEY|SECRET|PASSWORD|PASSWD|TOKEN)\S*\s*[:=]\s*).+$', '$1[REDACTED]'
    return $out
}

function Find-Tool {
    param(
        [Parameter(Mandatory)][string]$Name,
        [string[]]$FallbackPaths = @()
    )
    $cmd = Get-Command $Name -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    foreach ($p in $FallbackPaths) {
        if ($p -and (Test-Path $p)) { return $p }
    }
    return $null
}

function Get-ToolVersion {
    param([string]$Path)
    if (-not $Path) { return 'not found' }
    try {
        $out = & $Path --version 2>&1
        $first = ($out | Select-Object -First 1)
        if ($null -eq $first) { return '(no output)' }
        return (Protect-Secrets ($first.ToString().Trim()))
    } catch {
        return "error invoking: $($_.Exception.Message)"
    }
}

# ---------------------------------------------------------------------------
# Locate toolchain. Prefer PATH; fall back to the default rustup install
# location for machines where cargo/rustc are not on PATH for this shell.
# ---------------------------------------------------------------------------
$cargoBin  = Join-Path $env:USERPROFILE '.cargo\bin'
$NodePath  = Find-Tool -Name 'node'
$NpmPath   = Find-Tool -Name 'npm.cmd'
if (-not $NpmPath) { $NpmPath = Find-Tool -Name 'npm' }
$RustcPath = Find-Tool -Name 'rustc' -FallbackPaths @(Join-Path $cargoBin 'rustc.exe')
$CargoPath = Find-Tool -Name 'cargo' -FallbackPaths @(Join-Path $cargoBin 'cargo.exe')

# `npm run tauri -- build` shells out to `cargo` as a child process, which
# only sees $env:PATH, not the $CargoPath variable above. If cargo was only
# found via the rustup fallback location, make it visible to child
# processes for the remainder of this script's process tree so the Tauri
# CLI's own `cargo metadata`/`cargo build` calls can find it too.
if ($CargoPath -and (Test-Path $cargoBin) -and ($env:PATH -notlike "*$cargoBin*")) {
    $env:PATH = "$env:PATH;$cargoBin"
}

$NodeVersion  = Get-ToolVersion $NodePath
$NpmVersion   = Get-ToolVersion $NpmPath
$RustcVersion = Get-ToolVersion $RustcPath
$CargoVersion = Get-ToolVersion $CargoPath

# ---------------------------------------------------------------------------
# Git identity of the tree being verified.
# ---------------------------------------------------------------------------
Push-Location $RepoRoot
try {
    $CommitHash = ((git rev-parse HEAD 2>&1) | Out-String).Trim()
    $Branch     = ((git branch --show-current 2>&1) | Out-String).Trim()
    $StatusRaw  = @(git status --porcelain 2>&1)
} finally {
    Pop-Location
}

$UtcNow = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')

# ---------------------------------------------------------------------------
# Run each verification step. Output goes to its own redacted log file under
# test-results/ (never overwriting the pre-existing npm-test.log / etc. left
# by prior manual runs); build-info.txt only holds the summary + exit codes.
# ---------------------------------------------------------------------------
$Steps = New-Object System.Collections.Generic.List[object]

function Invoke-Step {
    param(
        [Parameter(Mandatory)][string]$Name,
        [Parameter(Mandatory)][string]$WorkingDir,
        [string]$Exe,
        [string[]]$Arguments,
        [Parameter(Mandatory)][string]$LogFile
    )

    $logPath = Join-Path $ResultsDir $LogFile
    $result = [ordered]@{
        Name       = $Name
        Command    = if ($Exe) { "$Exe $($Arguments -join ' ')" } else { "$Name (tool not found)" }
        LogFile    = $LogFile
        ExitCode   = $null
        Status     = 'skipped'
    }

    if (-not $Exe) {
        Set-Content -Path $logPath -Value "SKIPPED: required tool for '$Name' was not found on PATH or in the rustup default install location." -Encoding UTF8
        $result.Status = 'skipped-tool-missing'
        $obj = [pscustomobject]$result
        $Steps.Add($obj) | Out-Null
        return $obj
    }

    if (-not (Test-Path $WorkingDir)) {
        Set-Content -Path $logPath -Value "SKIPPED: working directory not found: $WorkingDir" -Encoding UTF8
        $result.Status = 'skipped-dir-missing'
        $obj = [pscustomobject]$result
        $Steps.Add($obj) | Out-Null
        return $obj
    }

    Push-Location $WorkingDir
    try {
        & $Exe @Arguments *> $logPath
        $exitCode = $LASTEXITCODE
    } catch {
        $exitCode = 1
        Add-Content -Path $logPath -Value "EXCEPTION: $($_.Exception.Message)"
    } finally {
        Pop-Location
    }

    if (Test-Path $logPath) {
        $raw = Get-Content -Path $logPath -Raw -ErrorAction SilentlyContinue
        Set-Content -Path $logPath -Value (Protect-Secrets $raw) -Encoding UTF8
    }

    $result.ExitCode = $exitCode
    $result.Status = if ($exitCode -eq 0) { 'pass' } else { 'fail' }
    $obj = [pscustomobject]$result
    $Steps.Add($obj) | Out-Null
    return $obj
}

Invoke-Step -Name 'npm test'              -WorkingDir $DesktopDir -Exe $NpmPath   -Arguments @('test', '--', '--run')      -LogFile 'verify-build-npm-test.log'    | Out-Null
Invoke-Step -Name 'npm run build'         -WorkingDir $DesktopDir -Exe $NpmPath   -Arguments @('run', 'build')             -LogFile 'verify-build-npm-build.log'   | Out-Null
Invoke-Step -Name 'cargo test'            -WorkingDir $TauriDir   -Exe $CargoPath -Arguments @('test')                     -LogFile 'verify-build-cargo-test.log'  | Out-Null
Invoke-Step -Name 'npm run tauri -- build' -WorkingDir $DesktopDir -Exe $NpmPath  -Arguments @('run', 'tauri', '--', 'build') -LogFile 'verify-build-tauri-build.log' | Out-Null

$FailedSteps  = @($Steps | Where-Object { $_.Status -eq 'fail' })
$MissingSteps = @($Steps | Where-Object { $_.Status -like 'skipped-*' })
$OverallStatus = if ($FailedSteps.Count -gt 0 -or $MissingSteps.Count -gt 0) { 'FAIL' } else { 'PASS' }

# ---------------------------------------------------------------------------
# Write build-info.txt
# ---------------------------------------------------------------------------
$lines = New-Object System.Collections.Generic.List[string]
$lines.Add('Nova Build Verification Report') | Out-Null
$lines.Add("Generated (UTC): $UtcNow") | Out-Null
$lines.Add('') | Out-Null
$lines.Add('This file is a regenerable verification artifact produced by scripts\verify-build.ps1.') | Out-Null
$lines.Add('Do not hand-edit it and do not commit it as part of a source change.') | Out-Null
$lines.Add('') | Out-Null
$lines.Add('Git') | Out-Null
$lines.Add("  commit: $CommitHash") | Out-Null
$lines.Add("  branch: $Branch") | Out-Null
$lines.Add('  status (git status --porcelain):') | Out-Null
if ($StatusRaw -and $StatusRaw.Count -gt 0) {
    foreach ($l in $StatusRaw) { $lines.Add("    $l") | Out-Null }
} else {
    $lines.Add('    (clean)') | Out-Null
}
$lines.Add('') | Out-Null
$lines.Add('Toolchain') | Out-Null
$lines.Add("  node:  $NodeVersion") | Out-Null
$lines.Add("  npm:   $NpmVersion") | Out-Null
$lines.Add("  rustc: $RustcVersion") | Out-Null
$lines.Add("  cargo: $CargoVersion") | Out-Null
$lines.Add('') | Out-Null
$lines.Add('Steps') | Out-Null
foreach ($step in $Steps) {
    $lines.Add("  [$($step.Status.ToUpper())] $($step.Name) (exit=$($step.ExitCode)) -> test-results\$($step.LogFile)") | Out-Null
}
$lines.Add('') | Out-Null
$lines.Add("Overall: $OverallStatus") | Out-Null

Set-Content -Path $BuildInfoPath -Value $lines -Encoding UTF8

Write-Host "Build verification written to $BuildInfoPath"
foreach ($step in $Steps) {
    Write-Host ("  [{0}] {1} (exit={2})" -f $step.Status.ToUpper(), $step.Name, $step.ExitCode)
}
Write-Host "Overall status: $OverallStatus"

if ($OverallStatus -ne 'PASS') {
    exit 1
}
exit 0
