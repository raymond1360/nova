#requires -Version 7.0
param([Parameter(Mandatory=$true)][string]$EvidenceDirectory)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (-not [IO.Path]::IsPathRooted($EvidenceDirectory) -or (Test-Path -LiteralPath $EvidenceDirectory)) { throw 'New absolute evidence directory required' }
New-Item -ItemType Directory -Path $EvidenceDirectory -ErrorAction Stop | Out-Null
$upload = Join-Path $EvidenceDirectory 'upload'
New-Item -ItemType Directory -Path $upload | Out-Null
$target = Join-Path $EvidenceDirectory 'cargo-target'
$clock = [Diagnostics.Stopwatch]::StartNew()
$totalMs = 1800000
$logLimit = 16777216 # per stream, local only; never uploaded
$rows = [Collections.Generic.List[object]]::new()
$status = 'FAILED'; $stage = 'BUILD'; $failure = 'CHECK_FAILED'
$cargo = (Get-Command cargo -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
function Invoke-Fixed([string]$Name, [string]$Executable, [string[]]$Vector) {
 $remaining = $totalMs - $clock.ElapsedMilliseconds
 if ($remaining -le 0) { $script:failure = 'TOTAL_DEADLINE'; throw 'Total deadline' }
 $info = [Diagnostics.ProcessStartInfo]::new()
 $info.FileName=$Executable; $info.WorkingDirectory=Join-Path $root 'apps/desktop/src-tauri'
 $info.UseShellExecute=$false; $info.CreateNoWindow=$true
 $info.RedirectStandardOutput=$true; $info.RedirectStandardError=$true
 $info.Environment['CARGO_TARGET_DIR']=$target
 $info.Environment['CARGO_NET_OFFLINE']='true'; $info.Environment['RUSTUP_AUTO_INSTALL']='0'
 foreach($arg in $Vector){$info.ArgumentList.Add($arg)}
 $p=[Diagnostics.Process]::new();$p.StartInfo=$info
 $row=[ordered]@{name=$Name;started_utc=[DateTime]::UtcNow.ToString('o');ended_utc=$null;exit_code=$null;state='START_FAILED'}
 $rows.Add($row)
 $writers=@();$tasks=@();$buffers=@();$counts=@(0L,0L);$ended=@($false,$false)
 try {
  foreach($suffix in @('stdout','stderr')) {
   $writers += [IO.File]::Open((Join-Path $EvidenceDirectory ($Name+'.'+$suffix+'.log')),[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::Read)
   $buffers += ,([char[]]::new(4096))
  }
  if(-not $p.Start()){throw 'Start failed'}
  $row.state='PARTIAL'
  $readers=@($p.StandardOutput,$p.StandardError)
  for($i=0;$i -lt 2;$i++){$tasks += $readers[$i].ReadAsync($buffers[$i],0,4096)}
  while(-not ($ended[0] -and $ended[1] -and $p.HasExited)) {
   if($clock.ElapsedMilliseconds -ge $totalMs){$script:failure='TOTAL_DEADLINE';throw 'Total deadline'}
   for($i=0;$i -lt 2;$i++) {
    if(-not $ended[$i] -and $tasks[$i].IsCompleted) {
     $n=$tasks[$i].GetAwaiter().GetResult()
     if($n -eq 0){$ended[$i]=$true;continue}
     $bytes=[Text.Encoding]::UTF8.GetBytes($buffers[$i],0,$n)
     if($counts[$i]+$bytes.Length -gt $logLimit){$script:failure='LOG_LIMIT';throw 'Log limit'}
     $writers[$i].Write($bytes,0,$bytes.Length);$counts[$i]+=$bytes.Length
     $tasks[$i]=$readers[$i].ReadAsync($buffers[$i],0,4096)
    }
   }
   [Threading.Thread]::Sleep(10)
  }
  if($clock.ElapsedMilliseconds -ge $totalMs){$script:failure='TOTAL_DEADLINE';throw 'Late completion'}
  $row.exit_code=$p.ExitCode;$row.state='COMPLETE'
 } finally {
  if($row.state -eq 'PARTIAL' -and -not $p.HasExited){$p.Kill($true);[void]$p.WaitForExit(5000);if($p.HasExited){$row.exit_code=$p.ExitCode}}
  foreach($writer in $writers){$writer.Flush($true);$writer.Dispose()}
  $row.ended_utc=[DateTime]::UtcNow.ToString('o');$p.Dispose()
 }
 return $row.exit_code
}
try {
 $code=Invoke-Fixed 'debug-build' $cargo @('build','--offline','--locked','--features','t1-api-eval','--bin','nova_t1_m0b_offline')
 if($code -ne 0){throw 'Debug build failed'}
 $stage='MOCK'
 $exe=Join-Path $target 'debug/nova_t1_m0b_offline.exe'
 $code=Invoke-Fixed 'mock-self-test' $exe @('--mock-self-test')
 if($code -ne 0){throw 'Mock failed'}
 $v=Get-Content -LiteralPath (Join-Path $EvidenceDirectory 'mock-self-test.stdout.log') -Raw | ConvertFrom-Json -AsHashtable
 # Strict types: strings such as "false" or missing/null fields cannot pass.
 if($v.outcome.success -isnot [bool] -or $v.outcome.success -ne $true -or
    $v.outcome.source -cne 'INJECTED' -or $v.native_executed -isnot [bool] -or $v.native_executed -ne $false -or
    $v.upstream_executed -isnot [bool] -or $v.upstream_executed -ne $false -or
    $v.outcome.execution_authorized -isnot [bool] -or $v.outcome.execution_authorized -ne $false){throw 'Mock contract failed'}
 $stage='DEFAULT_REQUIRED_FEATURES'
 $code=Invoke-Fixed 'default-required-features' $cargo @('build','--offline','--locked','--bin','nova_t1_m0b_offline')
 $err=[string](Get-Content -LiteralPath (Join-Path $EvidenceDirectory 'default-required-features.stderr.log') -Raw)
 if($code -ne 101 -or $err -notmatch '(?m)^error: target `nova_t1_m0b_offline` in package `nova` requires the features: `t1-api-eval`\r?$' -or $err -match 'could not compile'){throw 'Not exact required-features rejection'}
 $stage='RELEASE_GUARD'
 $code=Invoke-Fixed 'release-guard' $cargo @('check','--offline','--locked','--release','--features','t1-api-eval','--bin','nova_t1_m0b_offline','--message-format=json')
 $errors=@(Get-Content -LiteralPath (Join-Path $EvidenceDirectory 'release-guard.stdout.log') | ForEach-Object {
  if($_.StartsWith('{')){$j=ConvertFrom-Json -InputObject $_;if($j.reason -eq 'compiler-message' -and $j.message.level -eq 'error'){$j}}
 })
 if($code -ne 101 -or $errors.Count -ne 1 -or $errors[0].target.name -cne 'nova_lib' -or $errors[0].message.message -cne 't1-api-eval is debug-only and must never ship'){throw 'Not exact debug-only guard'}
 $status='PASS';$failure=$null
} finally {
 # Upload only rebuilt fixed-schema facts. No raw text, paths, environment, model output or arbitrary error messages.
 $inventory=@();foreach($name in @('debug-build','mock-self-test','default-required-features','release-guard')) {
  foreach($suffix in @('stdout','stderr')) {
   $file=$name+'.'+$suffix+'.log';$path=Join-Path $EvidenceDirectory $file
   $row=@($rows | Where-Object {$_.name -ceq $name})
   if(-not (Test-Path -LiteralPath $path -PathType Leaf)){$inventory+=@{name=$file;state='MISSING';bytes=$null;sha256=$null};continue}
   $size=(Get-Item -LiteralPath $path).Length
   $state=if($row.Count -gt 0 -and $row[0].state -eq 'START_FAILED'){'NOT_STARTED'}elseif($row.Count -eq 0 -or $row[0].state -ne 'COMPLETE'){'PARTIAL'}elseif($size -eq 0){'EMPTY'}else{'PRESENT_LOCAL_ONLY'}
   $inventory+=@{name=$file;state=$state;bytes=$size;sha256=(Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()}
  }
 }
 $summary=@{schema='M0B_BIN_CI_V1';status=$status;stage=$stage;failure=$failure;elapsed_ms=$clock.ElapsedMilliseconds;total_limit_ms=$totalMs;commands=@($rows.ToArray());mock_contract_verified=($stage -ne 'BUILD' -and $stage -ne 'MOCK');native_executed=$false;upstream_executed=$false;execution_authorized=$false;scope='PUBLIC_INJECTED_CHAIN_NOT_NB1_DRIVER'}
 foreach($pair in @(@('summary.json',$summary),@('log-inventory.json',@{schema='M0B_BIN_CI_LOGS_V1';per_stream_limit=$logLimit;files=$inventory}))) {
  $text=ConvertTo-Json -InputObject $pair[1] -Depth 8
  if([Text.Encoding]::UTF8.GetByteCount($text) -gt 65536){throw 'Fixed diagnostic output limit'}
  [IO.File]::WriteAllText((Join-Path $upload $pair[0]),$text,[Text.UTF8Encoding]::new($false))
 }
}
if($status -ne 'PASS'){throw 'M0B bin checks failed'}
$global:LASTEXITCODE=0
