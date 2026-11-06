#requires -Version 7.0
param([Parameter(Mandatory)][string]$EvidenceDirectory,[string]$TargetDirectory)
$ErrorActionPreference='Stop'
$repo=Split-Path -Parent $PSScriptRoot
if(-not [IO.Path]::IsPathRooted($EvidenceDirectory) -or (Test-Path -LiteralPath $EvidenceDirectory)){throw 'New absolute evidence directory required'}
New-Item -ItemType Directory -Path $EvidenceDirectory | Out-Null
if(-not $TargetDirectory){$TargetDirectory=Join-Path $EvidenceDirectory 'target'}
if(-not [IO.Path]::IsPathRooted($TargetDirectory)){throw 'Absolute target required'}
$cargo=(Get-Command cargo -CommandType Application | Select-Object -First 1).Source
$git=(Get-Command git -CommandType Application | Select-Object -First 1).Source
if(-not $cargo -or -not $git){throw 'Tool missing'}
$crate=Join-Path $repo 'apps/desktop/src-tauri'
$commandTemp=Join-Path $EvidenceDirectory 'command-temp';New-Item -ItemType Directory -Path $commandTemp | Out-Null
$clock=[Diagnostics.Stopwatch]::StartNew();$deadline=3600000
$rows=[Collections.Generic.List[object]]::new();$verdict='FAIL';$stage='BUILD'
$gitElapsedMs=0L
function Invoke-Fixed([string]$Name,[string]$Exe,[string[]]$Vector,[string]$Cwd=$crate,[switch]$FixtureGit) {
 if($clock.ElapsedMilliseconds -ge $deadline){throw 'TOTAL_DEADLINE'}
 $info=[Diagnostics.ProcessStartInfo]::new();$info.FileName=$Exe;$info.WorkingDirectory=$Cwd
 $info.UseShellExecute=$false;$info.CreateNoWindow=$true;$info.RedirectStandardOutput=$true;$info.RedirectStandardError=$true
 $info.Environment['CARGO_TARGET_DIR']=$TargetDirectory;$info.Environment['CARGO_NET_OFFLINE']='true'
 $info.Environment['TEMP']=$commandTemp;$info.Environment['TMP']=$commandTemp
 $info.Environment['RUSTUP_AUTO_INSTALL']='0';$info.Environment['NOVA_SH1_TEST_ROOT']=$EvidenceDirectory
 if($FixtureGit){
  $info.Environment.Clear();$info.Environment['SystemRoot']=$env:SystemRoot
  $info.Environment['PATH']=(Split-Path $git)+';'+$env:SystemRoot+'\System32'
  foreach($key in @('TEMP','TMP','HOME','USERPROFILE')){$info.Environment[$key]=$EvidenceDirectory}
  $info.Environment['GIT_CONFIG_NOSYSTEM']='1';$info.Environment['GIT_CONFIG_GLOBAL']=$empty
  $info.Environment['GIT_TERMINAL_PROMPT']='0'
  foreach($key in @('GIT_AUTHOR_NAME','GIT_COMMITTER_NAME')){$info.Environment[$key]='Nova Fixture'}
  foreach($key in @('GIT_AUTHOR_EMAIL','GIT_COMMITTER_EMAIL')){$info.Environment[$key]='fixture@invalid.example'}
  foreach($key in @('GIT_AUTHOR_DATE','GIT_COMMITTER_DATE')){$info.Environment[$key]='2026-01-01T00:00:00+00:00'}
 }
 foreach($a in $Vector){$info.ArgumentList.Add($a)}
 $beforeTemps=@(Get-ChildItem -LiteralPath $commandTemp -Directory -Filter 'nova-sh1-*' | ForEach-Object Name)
 $local=[Diagnostics.Stopwatch]::StartNew()
 $row=[ordered]@{name=$Name;state='START_FAILED';exit_code=$null;started_utc=[DateTime]::UtcNow.ToString('o');ended_utc=$null}
 $rows.Add($row);$p=[Diagnostics.Process]::new();$p.StartInfo=$info
 $writers=@();$buffers=@();$tasks=@();$done=@($false,$false);$sizes=@(0L,0L)
 try {
  foreach($suffix in @('stdout','stderr')){$writers+= [IO.File]::Open((Join-Path $EvidenceDirectory "$Name.$suffix.log"),[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::Read);$buffers+= ,([char[]]::new(4096))}
  if(-not $p.Start()){throw 'START_FAILED'};$row.state='PARTIAL'
  $readers=@($p.StandardOutput,$p.StandardError)
  for($i=0;$i -lt 2;$i++){$tasks+= $readers[$i].ReadAsync($buffers[$i],0,4096)}
  while(-not ($done[0] -and $done[1] -and $p.HasExited)){
   if($clock.ElapsedMilliseconds -ge $deadline){throw 'TOTAL_DEADLINE'}
   if($FixtureGit -and ($local.ElapsedMilliseconds -ge 5000 -or $script:gitElapsedMs+$local.ElapsedMilliseconds -ge 30000)){throw 'FIXTURE_GIT_DEADLINE'}
   for($i=0;$i -lt 2;$i++){
    if(-not $done[$i] -and $tasks[$i].IsCompleted){$n=$tasks[$i].GetAwaiter().GetResult();if($n -eq 0){$done[$i]=$true;continue}
     $b=[Text.Encoding]::UTF8.GetBytes($buffers[$i],0,$n);$sizes[$i]+=$b.Length
     if($sizes[$i] -gt 16777216){throw 'LOG_BOUND'};$writers[$i].Write($b,0,$b.Length);$tasks[$i]=$readers[$i].ReadAsync($buffers[$i],0,4096)
    }
   }
   [Threading.Thread]::Sleep(10)
  }
  $row.state='COMPLETE';$row.exit_code=$p.ExitCode
  if($Exe -ceq $bin -and @(Get-ChildItem -LiteralPath $commandTemp -Directory -Filter 'nova-sh1-*' | Where-Object {$_.Name -cnotin $beforeTemps}).Count -ne 0){throw 'BIN_TEMP_NOT_CLEANED'}
 } finally {
  if($row.state -eq 'PARTIAL' -and -not $p.HasExited){$p.Kill($true);[void]$p.WaitForExit(5000);if($p.HasExited){$row.exit_code=$p.ExitCode}}
  if($FixtureGit){$script:gitElapsedMs+=$local.ElapsedMilliseconds}
  foreach($w in $writers){$w.Flush($true);$w.Dispose()};$row.ended_utc=[DateTime]::UtcNow.ToString('o');$p.Dispose()
 }
 return $row.exit_code
}
function Require-Zero([string]$Name,[string]$Exe,[string[]]$Vector,[string]$Cwd=$crate,[switch]$FixtureGit){if((Invoke-Fixed $Name $Exe $Vector $Cwd -FixtureGit:$FixtureGit) -ne 0){throw "Fixed stage failed: $Name"}}
function Output-Json([string]$Name){Get-Content -LiteralPath (Join-Path $EvidenceDirectory "$Name.stdout.log") -Raw | ConvertFrom-Json -AsHashtable}
function Write-Json([string]$Path,$Value){[IO.File]::WriteAllText($Path,($Value|ConvertTo-Json -Depth 30),[Text.UTF8Encoding]::new($false))}
# Serial libtest output may contain inherited child stdout between the test header and its `ok`.
# Keep the named pending test; never infer a pass from aggregate counts.
function Read-ActualPassed([string[]]$Lines,[string[]]$Declared) {
 $allowed=[Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
 foreach($name in $Declared){if(-not $allowed.Add($name)){throw 'DUPLICATE_DECLARED_TEST'}}
 $done=[Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal);$pending=$null
 foreach($line in $Lines){
  if($line -cmatch '^test (.+?) \.\.\.(.*)$'){
   if($pending -or -not $allowed.Contains($Matches[1])){throw 'TEST_SEQUENCE_REJECTED'}
   $pending=$Matches[1];$suffix=$Matches[2]
   if($suffix -ceq ' ok'){if(-not $done.Add($pending)){throw 'DUPLICATE_TEST_RESULT'};$pending=$null}
  } elseif($line -ceq 'ok') {
   if(-not $pending -or -not $done.Add($pending)){throw 'ORPHAN_TEST_RESULT'};$pending=$null
  } elseif($line -ceq 'FAILED' -or $line -cmatch '^test .+ \.\.\. FAILED$'){throw 'FAILED_TEST_RESULT'}
 }
 if($pending){throw 'PARTIAL_TEST_RESULT'}
 return @($done | Sort-Object)
}

$bin=Join-Path $TargetDirectory 'debug/nova_scheduler_handoff.exe'
$relay=Join-Path $TargetDirectory 'debug/nova_scheduler_relay.exe'
$fake=Join-Path $TargetDirectory 'debug/nova_scheduler_relay_fake_cli.exe'
try {
 Require-Zero 'relay-debug-build' $cargo @('build','--offline','--locked','--features','scheduler-validation','--bin','nova_scheduler_relay','--bin','nova_scheduler_relay_fake_cli','--bin','nova_scheduler_handoff')
 $stage='SH1_AND_DEFAULT_REGRESSION'
 Require-Zero 'sh1-wrapper' (Get-Process -Id $PID).Path @('-NoProfile','-File',(Join-Path $PSScriptRoot 'test_scheduler_handoff.ps1'),'-EvidenceDirectory',(Join-Path $EvidenceDirectory 'sh1-regression'),'-TargetDirectory',$TargetDirectory) $repo
 $old=Get-Content (Join-Path $EvidenceDirectory 'sh1-regression/result.json') -Raw | ConvertFrom-Json -AsHashtable
 if($old.verdict -cne 'PASS' -or $old.default_regression_executed -isnot [bool] -or -not $old.default_regression_executed){throw 'INCOMPLETE_OLD_REGRESSION'}
 foreach($stem in @('scheduler','default')){
  $list=Get-Content (Join-Path $EvidenceDirectory "sh1-regression/$stem-list.stdout.log")
  $ok=Get-Content (Join-Path $EvidenceDirectory "sh1-regression/$stem-tests.stdout.log")
  $names=@($list | ForEach-Object {if($_ -cmatch '^(.+): test$'){$Matches[1]}} | Sort-Object)
  $passed=@(Read-ActualPassed $ok $names)
  if($names.Count -eq 0 -or (Compare-Object $names $passed)){throw 'TEST_NAME_SET_MISMATCH'}
  Write-Json (Join-Path $EvidenceDirectory "$stem-names.json") @{source='ACTUAL_LOCAL_TEST_OUTPUT';listed=$names;passed=$passed;count=$passed.Count}
 }
 $stage='RELAY_GUARDS'
 foreach($name in @('nova_scheduler_relay','nova_scheduler_relay_fake_cli')){
  $code=Invoke-Fixed "default-$name" $cargo @('build','--offline','--locked','--bin',$name)
  $err=[string](Get-Content (Join-Path $EvidenceDirectory "default-$name.stderr.log") -Raw)
  if($code -ne 101 -or $err -cnotmatch ('(?m)^error: target `'+$name+'` in package `nova` requires the features: `scheduler-validation`\r?$') -or $err -match 'could not compile'){throw 'REQUIRED_FEATURES_NOT_EXACT'}
  $code=Invoke-Fixed "release-$name" $cargo @('check','--offline','--locked','--release','--features','scheduler-validation','--bin',$name,'--message-format=json')
  $errors=@(Get-Content (Join-Path $EvidenceDirectory "release-$name.stdout.log") | ForEach-Object {if($_.StartsWith('{')){$v=ConvertFrom-Json $_;if($v.reason -ceq 'compiler-message' -and $v.message.level -ceq 'error'){$v}}})
  if($code -ne 101 -or $errors.Count -ne 1 -or $errors[0].target.name -cne 'nova_lib' -or $errors[0].message.message -cne 'scheduler-validation is debug-only and must never ship'){throw 'RELEASE_GUARD_NOT_EXACT'}
 }
 $stage='RELAY_THREE_COMMANDS'
 $fixture=Join-Path $EvidenceDirectory 'git-fixture';$reports=Join-Path $EvidenceDirectory 'reports';$owned=Join-Path $EvidenceDirectory 'v1-source';$v2=Join-Path $EvidenceDirectory 'v2-owned';$run=Join-Path $EvidenceDirectory 'relay-run'
 $hooks=Join-Path $EvidenceDirectory 'empty-hooks';$template=Join-Path $EvidenceDirectory 'empty-template';$empty=Join-Path $EvidenceDirectory 'empty-gitconfig'
 foreach($path in @($fixture,$reports,$hooks,$template)){New-Item -ItemType Directory -Path $path | Out-Null};[IO.File]::WriteAllText($empty,'')
 $prefix=@('--no-pager','--no-optional-locks','-c',"core.hooksPath=$hooks",'-c','commit.gpgSign=false','-c','tag.gpgSign=false','-c','core.fsmonitor=false','-c','core.autocrlf=false','-c','core.pager=cat','-c','color.ui=false')
 Require-Zero 'fixture-init' $git ($prefix+@('init',"--template=$template",'--initial-branch=fixture',$fixture)) $EvidenceDirectory -FixtureGit
 New-Item -ItemType Directory -Path (Join-Path $fixture 'pilot/src') | Out-Null
 $target=Join-Path $fixture 'pilot/src/normalize_label.rs';$text="pub fn normalize_label(s: &str) -> String {`n    s.to_string()`n}`n"
 [IO.File]::WriteAllText($target,$text,[Text.UTF8Encoding]::new($false))
 Require-Zero 'fixture-add' $git ($prefix+@('-C',$fixture,'add','--','pilot/src/normalize_label.rs')) $EvidenceDirectory -FixtureGit
 Require-Zero 'fixture-base-commit' $git ($prefix+@('-C',$fixture,'commit','--no-gpg-sign','--no-verify','-m','fixture-base')) $EvidenceDirectory -FixtureGit
 Require-Zero 'fixture-base' $git ($prefix+@('-C',$fixture,'rev-parse','--verify','HEAD^{commit}')) $EvidenceDirectory -FixtureGit
 $base=([string](Get-Content (Join-Path $EvidenceDirectory 'fixture-base.stdout.log') -Raw)).Trim()
 Require-Zero 'fixture-second-commit' $git ($prefix+@('-C',$fixture,'commit','--allow-empty','--no-gpg-sign','--no-verify','-m','fixture-second')) $EvidenceDirectory -FixtureGit
 $manifest=Join-Path $EvidenceDirectory 'manifest.json'
 Write-Json $manifest @{schema='nova.sh1.package/1';package_id='ar0-fixed-pilot';repo_path=$fixture;title='Fixed mock relay';goal='Trim and lowercase one label';allowed_paths=@('pilot/src/normalize_label.rs');acceptance_items=@('one in-memory proposal; no file application');pm_decision_ref='AR0 LOCAL MOCK ONLY';report_roots=@($reports);required_reports=@('DEVELOPMENT','REVIEW','CI');baseline=@{mode='DECLARED';base_sha=$base};eol_policy='false'}
 Require-Zero 'source-init' $bin @('init','--workspace',$owned,'--manifest',$manifest)
 $source=Output-Json 'source-init'
 $plan=Join-Path $EvidenceDirectory 'plan.json';$fixtureHash=(Get-FileHash $target -Algorithm SHA256).Hash.ToLowerInvariant()
 $dto=@{schema='nova.relay.plan/1';package_id='ar0-fixed-pilot';registration_revision=0;input_base_sha=$source.head.value;input_delivery_identity=$source.delivery_identity;target_path='pilot/src/normalize_label.rs';base_sha256=$fixtureHash;goal='Trim and lowercase one label';constraints=@('Keep signature');examples=@(' A -> a');develop_profile='codex-exec-0.162.1';review_profile='claude-print-2.1.197';model_develop='mock:ok';model_review='mock:ok';pm_decision_ref='AR0 LOCAL MOCK ONLY';limits=@{stage_ms=30000;total_ms=90000};mock=$false}
 Write-Json $plan $dto
 if((Invoke-Fixed 'real-false-rejected' $relay @('relay-init','--workspace',$v2,'--source',$owned,'--plan',$plan,'--run-root',$run)) -ne 2 -or (Output-Json 'real-false-rejected').error_code -cne 'REAL_PILOT_NOT_AUTHORIZED' -or (Test-Path $v2) -or (Test-Path $run)){throw 'REAL_NOT_PRELEASE_BLOCKED'}
 $dto.mock=$true;Write-Json $plan $dto
 Require-Zero 'relay-init' $relay @('relay-init','--workspace',$v2,'--source',$owned,'--plan',$plan,'--run-root',$run)
 Require-Zero 'relay-run' $relay @('relay-run','--workspace',$v2)
 $out=Output-Json 'relay-run'
 if($out.next_action -cne 'PM_REVIEW' -or $out.facts.Count -ne 2 -or $out.delivery_sha -ne $null -or $out.execution_authorized -isnot [bool] -or $out.execution_authorized){throw 'RELAY_RESULT_REJECTED'}
 Require-Zero 'relay-status' $relay @('relay-status','--workspace',$v2)
 Require-Zero 'relay-no-replay' $relay @('relay-run','--workspace',$v2)
 $again=Output-Json 'relay-no-replay';if(($out.facts|ConvertTo-Json -Depth 20 -Compress) -cne ($again.facts|ConvertTo-Json -Depth 20 -Compress)){throw 'RELAY_REPLAY'}
 if((Get-FileHash $target -Algorithm SHA256).Hash.ToLowerInvariant() -cne $fixtureHash){throw 'CANDIDATE_APPLIED'}
 if((Invoke-Fixed 'v2-history-not-cloned' $relay @('relay-init','--workspace',(Join-Path $EvidenceDirectory 'never-copy'),'--source',$v2,'--plan',$plan,'--run-root',(Join-Path $EvidenceDirectory 'never-copy-run'))) -ne 2 -or (Output-Json 'v2-history-not-cloned').error_code -cne 'SOURCE_REGISTRATION_NOT_FRESH' -or (Test-Path (Join-Path $EvidenceDirectory 'never-copy'))){throw 'HISTORY_CLONE_NOT_REJECTED'}
 $verdict='PASS'
} finally {
 $inventory=@(Get-ChildItem -LiteralPath $EvidenceDirectory -File | Where-Object {$_.Extension -eq '.log'} | ForEach-Object {@{name=$_.Name;bytes=$_.Length;sha256=(Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()}})
 Write-Json (Join-Path $EvidenceDirectory 'result.json') @{schema='nova.relay.local-validation/1';verdict=$verdict;stage=$stage;commands=$rows;logs=$inventory;mock=$true;real_execution_authorized=$false;github_verified=$false}
}
if($verdict -cne 'PASS'){exit 1}
