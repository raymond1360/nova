param([Parameter(Mandatory)][string]$RunRoot, [int]$TimeoutSeconds = 240)
$ErrorActionPreference = 'Stop'
$run = (Resolve-Path -LiteralPath $RunRoot).Path
if (!(Test-Path -LiteralPath (Join-Path $run 'ISOLATED-VALIDATION.json'))) { throw 'Missing isolated runtime marker' }
if ((Split-Path (Split-Path $run -Parent) -Leaf) -notlike 'wp2a-desktop-validation-*') { throw 'Unexpected audit parent' }
$deadline = (Get-Date).AddSeconds($TimeoutSeconds)
while ((Get-Date) -lt $deadline) {
    $log = Join-Path $run 'checkpoints.jsonl'
    if (Test-Path -LiteralPath $log) {
        foreach ($line in Get-Content -LiteralPath $log) {
            try { $checkpoint = ($line | ConvertFrom-Json -ErrorAction Stop).checkpoint } catch { continue } # partial append
            if ($checkpoint.name -notmatch '^[a-zA-Z0-9_-]+$') { throw 'Unsafe checkpoint name' }
            $ack = Join-Path $run ('ack-' + $checkpoint.name)
            if (!(Test-Path -LiteralPath $ack)) {
                [IO.File]::WriteAllText($ack, 'AUTOMATED ACK: no screenshot claimed; DOM/database checkpoint retained')
                Write-Output ($checkpoint | Select-Object name,details | ConvertTo-Json -Depth 6 -Compress)
            }
        }
    }
    $result = Join-Path $run 'driver-result.jsonl'
    if (Test-Path -LiteralPath $result) { Get-Content -LiteralPath $result; exit 0 }
    Start-Sleep -Milliseconds 500
}
throw 'Validation driver did not finish within timeout'
