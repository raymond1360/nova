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
$clock=[Diagnostics.Stopwatch]::StartNew();$deadline=5400000
$rows=[Collections.Generic.List[object]]::new();$verdict='FAIL';$stage='BUILD'
$gitElapsedMs=0L
function Invoke-Fixed([string]$Name,[string]$Exe,[string[]]$Vector,[string]$Cwd=$crate,[switch]$FixtureGit,[string]$FakeHash) {
 if($clock.ElapsedMilliseconds -ge $deadline){throw 'TOTAL_DEADLINE'}
 $info=[Diagnostics.ProcessStartInfo]::new();$info.FileName=$Exe;$info.WorkingDirectory=$Cwd
 $info.UseShellExecute=$false;$info.CreateNoWindow=$true;$info.RedirectStandardOutput=$true;$info.RedirectStandardError=$true
 $info.Environment['CARGO_TARGET_DIR']=$TargetDirectory;$info.Environment['CARGO_NET_OFFLINE']='true'
 $info.Environment['TEMP']=$commandTemp;$info.Environment['TMP']=$commandTemp
 [void]$info.Environment.Remove('NOVA_AR0_TEST_FAKE_SHA256');if($FakeHash){$info.Environment['NOVA_AR0_TEST_FAKE_SHA256']=$FakeHash};
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
function Require-Zero([string]$Name,[string]$Exe,[string[]]$Vector,[string]$Cwd=$crate,[switch]$FixtureGit,[string]$FakeHash){if((Invoke-Fixed $Name $Exe $Vector $Cwd -FixtureGit:$FixtureGit -FakeHash:$FakeHash) -ne 0){throw "Fixed stage failed: $Name"}}
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
   if($suffix -ceq ' ok'){if(-not $done.Add($pending)){throw 'DUPLICATE_TEST_RESULT'};$pending=$null}elseif($suffix.StartsWith(' ignored')){$pending=$null}
  } elseif($line -ceq 'ok') {
   if(-not $pending -or -not $done.Add($pending)){throw 'ORPHAN_TEST_RESULT'};$pending=$null
  } elseif($line -ceq 'FAILED' -or $line -cmatch '^test .+ \.\.\. FAILED$'){throw 'FAILED_TEST_RESULT'}
 }
 if($pending){throw 'PARTIAL_TEST_RESULT'}
 return @($done | Sort-Object)
}


$bin=Join-Path $TargetDirectory 'debug/nova_scheduler_api_injected.exe'
$pwsh=(Get-Command pwsh -CommandType Application|Select-Object -First 1).Source
if(-not $pwsh){throw 'PWSH_MISSING'}
$tests=$null;$failure='INCOMPLETE';$verdict='FAIL'
try {
 Require-Zero 'old-relay-regression' $pwsh @('-NoProfile','-File',(Join-Path $PSScriptRoot 'test_scheduler_relay.ps1'),'-EvidenceDirectory',(Join-Path $EvidenceDirectory 'old-relay'),'-TargetDirectory',$TargetDirectory) $repo
 $prior=Get-Content (Join-Path $EvidenceDirectory 'old-relay/result.json') -Raw|ConvertFrom-Json -AsHashtable
 if($prior.verdict -cne 'PASS' -or @($prior.commands).Count -ne 42 -or @($prior.commands|Where-Object state -cne 'COMPLETE').Count){throw 'OLD_RELAY_INCOMPLETE'}
 $sh1=Get-Content (Join-Path $EvidenceDirectory 'old-relay/sh1-regression/result.json') -Raw|ConvertFrom-Json -AsHashtable
 if($sh1.verdict -cne 'PASS' -or $sh1.default_regression_executed -isnot [bool] -or -not $sh1.default_regression_executed){throw 'DEFAULT_REGRESSION_INCOMPLETE'}
 $tests=Get-Content (Join-Path $EvidenceDirectory 'old-relay/default-names.json') -Raw|ConvertFrom-Json -AsHashtable
 $scheduler=Get-Content (Join-Path $EvidenceDirectory 'old-relay/scheduler-names.json') -Raw|ConvertFrom-Json -AsHashtable
 $explicit=Get-Content (Join-Path $EvidenceDirectory 'old-relay/explicit-names.json') -Raw|ConvertFrom-Json -AsHashtable
 if(@($tests.ignored).Count -ne 19 -or @($scheduler.ignored).Count -ne 19){throw 'IGNORED_SET_REJECTED'}
 # The unchanged relay script binds each exact ignored invocation to its freshly built fake hash.
 Require-Zero 'api-debug-build' $cargo @('build','--offline','--locked','--features','scheduler-validation','--bin','nova_scheduler_api_injected')
 $exit=Invoke-Fixed 'api-default-guard' $cargo @('build','--offline','--locked','--bin','nova_scheduler_api_injected')
 $err=[string](Get-Content (Join-Path $EvidenceDirectory 'api-default-guard.stderr.log') -Raw)
 if($exit -ne 101 -or $err -notmatch 'target `nova_scheduler_api_injected`.*requires the features: `scheduler-validation`' -or $err -match 'error\[E\d+\]|could not compile'){throw 'REQUIRED_FEATURE_GUARD_REJECTED'}
 $exit=Invoke-Fixed 'api-release-guard' $cargo @('build','--offline','--locked','--release','--features','scheduler-validation','--bin','nova_scheduler_api_injected')
 $err=[string](Get-Content (Join-Path $EvidenceDirectory 'api-release-guard.stderr.log') -Raw)
 if($exit -ne 101 -or @([regex]::Matches($err,'(?m)^error: scheduler-validation is debug-only and must never ship')).Count -ne 1 -or $err -match 'error\[E\d+\]'){throw 'RELEASE_GUARD_REJECTED'}
 $workspace=Join-Path $EvidenceDirectory 'api-workspace';$run=Join-Path $EvidenceDirectory 'api-run-root';$plan=Join-Path $EvidenceDirectory 'api-request.json'
 $fixture=[Text.Encoding]::UTF8.GetBytes("pub fn normalize_label(s: &str) -> String {`n    s.to_string()`n}`n")
 $baseHash=[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($fixture)).ToLowerInvariant()
 Write-Json $plan ([ordered]@{schema='nova.scheduler.api-plan-request/1';mode='API_INJECTED_ONLY';package_id='api-injected-script';registration_revision=0;input_base_sha=('3'*40);target_path='pilot/src/normalize_label.rs';base_sha256=$baseHash;goal='Trim and lowercase';constraints=@('Keep the signature');examples=@();protocol='API_JSON_INJECTED_V1';model_develop='injected-develop-v1';model_review='injected-review-v1';limits=@{stage_ms=45000;total_ms=120000;input_tokens=8192;output_tokens=4096};pm_decision_ref='INJECTED fixture reference, not real execution approval'})
 Require-Zero 'api-init' $bin @('api-init','--workspace',$workspace,'--plan',$plan,'--run-root',$run)
 Require-Zero 'api-run' $bin @('api-run','--workspace',$workspace)
 $v=Output-Json 'api-run'
 if($v.mode -cne 'API_INJECTED_ONLY' -or $v.source -cne 'INJECTED' -or $v.next_action -cne 'PM_REVIEW' -or $v.facts.Count -ne 2 -or @($v.facts|Where-Object {$_.state -cne 'RESPONSE_RECEIVED' -or $_.facts.error_code -cne 'NONE' -or $_.evidence.state -cne 'COMPLETE'}).Count){throw 'API_TWO_STAGE_INCOMPLETE'}
 foreach($key in @('native_executed','network_executed','execution_authorized','next_execution_authorized')){if($v[$key] -isnot [bool] -or $v[$key]){throw 'API_MODE_IDENTITY_REJECTED'}}
 Require-Zero 'api-reopen-status' $bin @('api-status','--workspace',$workspace)
 Require-Zero 'api-no-replay' $bin @('api-run','--workspace',$workspace)
 $again=Output-Json 'api-no-replay'
 if(($again.facts|ConvertTo-Json -Depth 30 -Compress) -cne ($v.facts|ConvertTo-Json -Depth 30 -Compress)){throw 'API_REPLAY'}
 if((Get-FileHash (Join-Path $run 'fixture/pilot/src/normalize_label.rs') -Algorithm SHA256).Hash.ToLowerInvariant() -cne $baseHash){throw 'CANDIDATE_APPLIED'}
 $handoff=Join-Path $TargetDirectory 'debug/nova_scheduler_handoff.exe';$relay=Join-Path $TargetDirectory 'debug/nova_scheduler_relay.exe'
 $before=(Get-FileHash (Join-Path $workspace 'handoff.sqlite3')).Hash
 foreach($verb in @('status','handoff','import','index-only')){
  $vector=if($verb -eq 'index-only'){@('import','--workspace',$workspace,'--report',$plan,'--index-only')}elseif($verb -eq 'import'){@('import','--workspace',$workspace,'--report',$plan)}else{@($verb,'--workspace',$workspace)}
  $name="legacy-$verb-v3";$exit=Invoke-Fixed $name $handoff $vector
  if($exit -ne 2 -or (Output-Json $name).error_code -cne 'API_STORE_COMMAND_REQUIRED'){throw 'LEGACY_V3_OPEN_NOT_REJECTED'}
 }
 foreach($verb in @('relay-status','relay-run','relay-init')){
  $vector=if($verb -eq 'relay-init'){@($verb,'--source',$workspace,'--workspace',(Join-Path $EvidenceDirectory 'never-relay'),'--plan',(Join-Path $EvidenceDirectory 'old-relay/plan.json'),'--run-root',(Join-Path $EvidenceDirectory 'never-run'))}else{@($verb,'--workspace',$workspace)}
  $name="legacy-$verb-v3";$exit=Invoke-Fixed $name $relay $vector
  if($exit -ne 2 -or (Output-Json $name).error_code -cne 'API_STORE_COMMAND_REQUIRED'){throw 'RELAY_V3_OPEN_NOT_REJECTED'}
 }
 $oldManifest=Join-Path $EvidenceDirectory 'old-relay/sh1-regression/manifest.json'
 $legacyManifest=Get-Content $oldManifest -Raw|ConvertFrom-Json -AsHashtable
 Require-Zero 'legacy-current-head' $git @('rev-parse','HEAD') $legacyManifest.repo_path
 $legacyManifest.baseline=@{mode='DECLARED';base_sha=([string](Get-Content (Join-Path $EvidenceDirectory 'legacy-current-head.stdout.log') -Raw)).Trim()}
 $oldManifest=Join-Path $EvidenceDirectory 'legacy-existing-root-manifest.json';Write-Json $oldManifest $legacyManifest
 $exit=Invoke-Fixed 'legacy-init-existing-v3' $handoff @('init','--workspace',$workspace,'--manifest',$oldManifest)
 if($exit -ne 2 -or (Output-Json 'legacy-init-existing-v3').error_code -cne 'CREATE_NEW_REJECTED'){throw 'LEGACY_INIT_EXISTING_NOT_REJECTED'}
 if((Get-FileHash (Join-Path $workspace 'handoff.sqlite3')).Hash -cne $before){throw 'LEGACY_V3_WROTE'}
 $verdict='PASS';$failure='NONE'
} catch { $failure=if($_.Exception.Message -cmatch '^[A-Z0-9_]+$'){$_.Exception.Message}else{'LOCAL_CHECK_FAILED'};throw
} finally {
 # Reconstruct only closed fields. Raw logs stay private, never in the upload manifest.
 $fixed=@{schema='nova.scheduler.api-validation/1';mode='API_INJECTED_ONLY';source='ACTUAL_LOCAL_INJECTED_VALIDATION';source_sha=(& $git -C $repo rev-parse HEAD);verdict=$verdict;failure_code=$failure;network_executed=$false;real_execution_authorized=$false;github_verified=$false}
 $checks=@{schema='nova.scheduler.api-checks/1';commands=@($rows|ForEach-Object {@{name=$_.name;state=$_.state;exit_code=$_.exit_code}});old_default_regression_required=$true}
 $names=@{schema='nova.scheduler.api-tests/1';source='ACTUAL_LOCAL_TEST_OUTPUT';listed=$tests.listed;passed=$tests.passed;ignored=$tests.ignored}
 foreach($pair in @(@('result.json',$fixed,65536),@('checks.json',$checks,131072),@('tests.json',$names,262144))){$path=Join-Path $EvidenceDirectory $pair[0];Write-Json $path $pair[1];if((Get-Item $path).Length -gt $pair[2]){throw 'SUMMARY_BOUND'}}
 $files=@('result.json','checks.json','tests.json')|ForEach-Object {$p=Join-Path $EvidenceDirectory $_;@{name=$_;bytes=(Get-Item $p).Length;sha256=(Get-FileHash $p -Algorithm SHA256).Hash.ToLowerInvariant()}}
 Write-Json (Join-Path $EvidenceDirectory 'files.json') @{schema='nova.scheduler.api-files/1';files=@($files)}
 if((Get-Item (Join-Path $EvidenceDirectory 'files.json')).Length -gt 16384){throw 'MANIFEST_BOUND'}
}
if($verdict -cne 'PASS'){exit 1}
