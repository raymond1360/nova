# Fixed outer supervisor; not executed by the pure mock development pack.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
if ($args.Count -ne 0) { throw 'No arguments accepted' }
$root = 'C:\NovaM0B\run'
$inputRoot = 'D:\NovaM0BInput'
$policy = Get-Content -LiteralPath "$inputRoot\resource-policy.json" -Raw | ConvertFrom-Json
if ($policy.prepared -ne $true -or $policy.execution_authorized -ne $true -or
    $null -eq $policy.facility_receipt -or $policy.controller_sha256 -cnotmatch '^[0-9a-f]{64}$') {
    throw 'BLOCKED: missing approved resources/facilities; no controller starts'
}
$exe = "$inputRoot\nova_t1_m0b_offline.exe"
if ((Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash.ToLowerInvariant() -cne $policy.controller_sha256) {
    throw 'BLOCKED: controller identity'
}
$attempt = [IO.File]::Open("$root\supervisor-attempt", [IO.FileMode]::CreateNew,
    [IO.FileAccess]::Write, [IO.FileShare]::Read)
$attempt.Dispose()
$start = [Diagnostics.ProcessStartInfo]::new($exe)
$start.Arguments = '--offline-probe-once'
$start.UseShellExecute = $false
$start.WorkingDirectory = $root
$start.EnvironmentVariables.Clear()
$start.EnvironmentVariables['SystemRoot'] = $env:SystemRoot
$start.EnvironmentVariables['WINDIR'] = $env:SystemRoot
$start.EnvironmentVariables['TEMP'] = "$root\agent\tmp"
$start.EnvironmentVariables['TMP'] = "$root\agent\tmp"
$p = [Diagnostics.Process]::Start($start)
$watch = [Diagnostics.Stopwatch]::StartNew()
while (-not $p.HasExited -and $watch.ElapsedMilliseconds -lt 240000) {
    Start-Sleep -Milliseconds 50
}
if (-not $p.HasExited) {
    # Controller loss is only kill-on-close fallback, never an observed Job-zero receipt.
    $p.Kill()
    $status = @{state='PARTIAL'; job='UNKNOWN'; reason='outer deadline'; cas='consult journal; no automatic recovery'}
} else {
    $status = @{state='REVIEW_REQUIRED'; controller_exit=$p.ExitCode; job='consult explicit query receipt'}
}
# A new fixed status record, never a replacement for native receipts.
$f = [IO.File]::Open("$root\supervisor-result.json", [IO.FileMode]::CreateNew,
    [IO.FileAccess]::Write, [IO.FileShare]::Read)
try {
    $bytes = [Text.Encoding]::UTF8.GetBytes(($status | ConvertTo-Json -Compress))
    $f.Write($bytes,0,$bytes.Length); $f.Flush($true)
} finally { $f.Dispose(); $p.Dispose() }
