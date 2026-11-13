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
function Invoke-Fixed([string]$Name,[string]$Exe,[string[]]$Vector,[string]$Cwd=$crate,[string]$FakeHash) {
 if($clock.ElapsedMilliseconds -ge $deadline){throw 'TOTAL_DEADLINE'}
 $info=[Diagnostics.ProcessStartInfo]::new();$info.FileName=$Exe;$info.WorkingDirectory=$Cwd
 $info.UseShellExecute=$false;$info.CreateNoWindow=$true;$info.RedirectStandardOutput=$true;$info.RedirectStandardError=$true
 $info.Environment['CARGO_TARGET_DIR']=$TargetDirectory;$info.Environment['CARGO_NET_OFFLINE']='true'
 $info.Environment['TEMP']=$commandTemp;$info.Environment['TMP']=$commandTemp
 [void]$info.Environment.Remove('NOVA_AR0_TEST_FAKE_SHA256');if($FakeHash){$info.Environment['NOVA_AR0_TEST_FAKE_SHA256']=$FakeHash};
 $info.Environment['RUSTUP_AUTO_INSTALL']='0';$info.Environment['NOVA_SH1_TEST_ROOT']=$EvidenceDirectory
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
  foreach($w in $writers){$w.Flush($true);$w.Dispose()};$row.ended_utc=[DateTime]::UtcNow.ToString('o');$p.Dispose()
 }
 return $row.exit_code
}
function Require-Zero([string]$Name,[string]$Exe,[string[]]$Vector,[string]$Cwd=$crate,[string]$FakeHash){if((Invoke-Fixed $Name $Exe $Vector $Cwd -FakeHash:$FakeHash) -ne 0){throw "Fixed stage failed: $Name"}}
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



function SetHash($Items){$a=[string[]]@($Items);[Array]::Sort($a,[StringComparer]::Ordinal);$b=[Text.Encoding]::UTF8.GetBytes(($a -join "`n")+"`n");return [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($b)).ToLowerInvariant()}
function Assert-Set($Listed,$Passed,$Ignored) {
 foreach($a in @(@($Listed),@($Passed),@($Ignored))){if($a.Count -eq 0 -or @($a|Sort-Object -Unique).Count -ne $a.Count){throw 'API_SET_DUPLICATE_OR_EMPTY'}}
 if(Compare-Object (@($Listed)|Sort-Object) (@(@($Passed)+@($Ignored))|Sort-Object)){throw 'API_SET_PARTITION'}
 if(@($Passed|Where-Object {$_ -cin $Ignored}).Count){throw 'API_IGNORED_EXECUTED'}
}
function Get-DbIdentity([string]$Workspace) {
 $out=@();foreach($n in @('handoff.sqlite3','handoff.sqlite3-wal','handoff.sqlite3-shm')){
  $p=Join-Path $Workspace $n
  if(Test-Path -LiteralPath $p){$i=Get-Item -LiteralPath $p -Force;if($i.Attributes -band [IO.FileAttributes]::ReparsePoint){throw 'DB_LINK_REJECTED'};$out+=@{name=$n;exists=$true;bytes=$i.Length;sha256=(Get-FileHash $p -Algorithm SHA256).Hash.ToLowerInvariant()}}
  else{$out+=@{name=$n;exists=$false;bytes=$null;sha256=$null}}
 };return ($out|ConvertTo-Json -Depth 5 -Compress)
}
function Assert-Reopened($First,$Reopened,$Again){
 if(($First|ConvertTo-Json -Depth 50 -Compress) -cne ($Reopened|ConvertTo-Json -Depth 50 -Compress) -or ($First|ConvertTo-Json -Depth 50 -Compress) -cne ($Again|ConvertTo-Json -Depth 50 -Compress)){throw 'API_REOPEN_MISMATCH'}
 if($First.facts.Count -ne 2 -or @($First.facts.attempt_id|Sort-Object -Unique).Count -ne 2 -or @($First.facts|Where-Object {$_.facts.dispatch_invoked -isnot [bool] -or -not $_.facts.dispatch_invoked}).Count){throw 'API_ATTEMPT_SET'}
}

$RequiredApiTests=@('scheduler::tests::api_injected::api_complete_two_stage_fixture_unchanged_and_restart_no_send','scheduler::tests::api_injected::api_double_connection_reservation_and_cas_conflict','scheduler::tests::api_injected::api_evidence_failure_survives_reopen_no_review','scheduler::tests::api_injected::api_existing_workspace_init_rejects_without_writing','scheduler::tests::api_injected::api_f1_code_text_and_protocol_content_boundaries','scheduler::tests::api_injected::api_failed_develop_exact_codes_never_review','scheduler::tests::api_injected::api_file_drift_preserves_historical_facts_and_blocks_next','scheduler::tests::api_injected::api_limits_depth_duplicate_and_unknown_request','scheduler::tests::api_injected::api_partial_init_preserved_and_not_auto_repaired','scheduler::tests::api_injected::api_real_mode_rejected_before_root_or_lease','scheduler::tests::api_injected::api_request_binds_after_guard_and_differs_across_roots','scheduler::tests::api_injected::api_request_meta_forgery_rejected_before_lease','scheduler::tests::api_injected::api_request_rejects_final_delivery_and_plan_id_before_roots','scheduler::tests::api_injected::api_review_claims_are_not_acceptance_or_auto_repair','scheduler::tests::api_injected::api_secret_failure_and_pre_cancel_zero_reservation','scheduler::tests::api_injected::api_secret_unicode_all_decoded_and_raw_paths_reject','scheduler::tests::api_injected::api_simultaneous_connections_only_one_reservation','scheduler::tests::api_injected::api_stale_states_read_only_no_permit_or_replay','scheduler::tests::api_injected::api_tampered_delivery_or_ddl_rejected','scheduler::tests::api_injected::api_unknown_tokens_zero_send','scheduler::tests::api_injected::api_v1_and_v2_api_open_refuse_no_write','scheduler::tests::api_injected::api_v3_ddl_golden_and_old_open_zero_writes','scheduler::tests::ar0::api_ea7_behavior_byte_goldens','scheduler::deepseek_transport::tests::deepseek_approval_default_close_and_snapshot_drift','scheduler::deepseek_transport::tests::deepseek_evidence_failure_stops_review','scheduler::deepseek_transport::tests::deepseek_failed_develop_unknown_and_no_review','scheduler::deepseek_transport::tests::deepseek_fake_credential_configuration_only_uses_dedicated_slot','scheduler::deepseek_transport::tests::deepseek_modified_http_request_and_cas_are_rejected','scheduler::deepseek_transport::tests::deepseek_no_approval_zero_reads_and_binding_rejects','scheduler::deepseek_transport::tests::deepseek_permit_once_and_cancel_before_http','scheduler::deepseek_transport::tests::deepseek_persistence_failure_restart_is_read_only','scheduler::deepseek_transport::tests::deepseek_proxy_plan_and_old_store_are_not_real_authorizations','scheduler::deepseek_transport::tests::deepseek_secret_echo_never_persists_body_or_body_hash','scheduler::deepseek_transport::tests::deepseek_timeout_cancel_resources_and_size_stop_without_replay','scheduler::deepseek_transport::tests::deepseek_two_contexts_usage_and_restart_no_send','scheduler::deepseek_transport::tests::deepseek_wire_metadata_secret_and_usage_rejections','scheduler::deepseek_transport::tests::ds_r1_actual_plan_and_fixed_policy_are_visible','scheduler::deepseek_transport::tests::ds_r1_confirmation_initializes_displayed_object_without_reread','scheduler::deepseek_transport::tests::ds_r1_rejects_missing_direct_wrong_digest_eof_and_display_error')
$bin=Join-Path $TargetDirectory 'debug/nova_scheduler_api_injected.exe'
$pwsh=(Get-Command pwsh -CommandType Application|Select-Object -First 1).Source
if(-not $pwsh){throw 'PWSH_MISSING'}
$deepseekEntry=$null;$tests=$null;$scheduler=$null;$explicit=$null;$prior=$null;$sh1=$null;$v=$null;$again=$null;$before=$null;$failure='INCOMPLETE';$verdict='FAIL'
try {
 Require-Zero 'old-relay-regression' $pwsh @('-NoProfile','-File',(Join-Path $PSScriptRoot 'test_scheduler_relay.ps1'),'-EvidenceDirectory',(Join-Path $EvidenceDirectory 'old-relay'),'-TargetDirectory',$TargetDirectory) $repo
 $prior=Get-Content (Join-Path $EvidenceDirectory 'old-relay/result.json') -Raw|ConvertFrom-Json -AsHashtable
 if($prior.verdict -cne 'PASS' -or @($prior.commands).Count -ne 42 -or @($prior.commands|Where-Object state -cne 'COMPLETE').Count){throw 'OLD_RELAY_INCOMPLETE'}
 $sh1=Get-Content (Join-Path $EvidenceDirectory 'old-relay/sh1-regression/result.json') -Raw|ConvertFrom-Json -AsHashtable
 if($sh1.verdict -cne 'PASS' -or $sh1.default_regression_executed -isnot [bool] -or -not $sh1.default_regression_executed){throw 'DEFAULT_REGRESSION_INCOMPLETE'}
 $tests=Get-Content (Join-Path $EvidenceDirectory 'old-relay/default-names.json') -Raw|ConvertFrom-Json -AsHashtable
 $scheduler=Get-Content (Join-Path $EvidenceDirectory 'old-relay/scheduler-names.json') -Raw|ConvertFrom-Json -AsHashtable
 $explicit=Get-Content (Join-Path $EvidenceDirectory 'old-relay/explicit-names.json') -Raw|ConvertFrom-Json -AsHashtable
 if((SetHash $tests.listed) -cne '796b1d2a7612084fed569a92d8f00255b6999a13f983eb880128e77af95e7f1a'){throw 'API_FROZEN_DEFAULT_LISTED'}
 if((SetHash $tests.passed) -cne '3757b24ce26719d3213fd059603fdb743b772ad1a3467310b8c37c8fd690856a'){throw 'API_FROZEN_DEFAULT_PASSED'}
 if((SetHash $tests.ignored) -cne 'a39bbead332b9d661508d9a4d3a81dbf78a7473b535e6e470c0d76e3738d9611'){throw 'API_FROZEN_DEFAULT_IGNORED'}
 if((SetHash $scheduler.listed) -cne 'fac824972e7ac8ae7e11a4c43f7097c5b1bb589211606ff2ca8a5930f40d7d7c'){throw 'API_FROZEN_SCHEDULER_LISTED'}
 if((SetHash $scheduler.passed) -cne '7b635760ea37f191fa64136a844ab17415714df9a3b5b86cc20dc36a0c5bffaa'){throw 'API_FROZEN_SCHEDULER_PASSED'}
 if((SetHash $scheduler.ignored) -cne 'a39bbead332b9d661508d9a4d3a81dbf78a7473b535e6e470c0d76e3738d9611'){throw 'API_FROZEN_SCHEDULER_IGNORED'}
 Assert-Set $tests.listed $tests.passed $tests.ignored
 Assert-Set $scheduler.listed $scheduler.passed $scheduler.ignored
 foreach($name in $RequiredApiTests){if($name -cnotin $tests.passed -or $name -cnotin $scheduler.passed){throw 'API_REQUIRED_TEST_MISSING'}}
 if(Compare-Object (@($tests.ignored)|Sort-Object) (@($explicit.passed)|Sort-Object)){throw 'API_EXPLICIT_SET'}
 if(@($explicit.passed|Sort-Object -Unique).Count -ne 19){throw 'API_EXPLICIT_DUPLICATE'}
 if(@($tests.ignored).Count -ne 19 -or @($scheduler.ignored).Count -ne 19){throw 'IGNORED_SET_REJECTED'}
 # Test coverage is reused from the actual default/scheduler passed sets above; do not rerun subsets.
 Require-Zero 'deepseek-debug-build' $cargo @('build','--offline','--locked','--features','scheduler-validation','--bin','nova_scheduler_api_real')
 $exit=Invoke-Fixed 'deepseek-default-guard' $cargo @('build','--offline','--locked','--bin','nova_scheduler_api_real')
 $err=[string](Get-Content (Join-Path $EvidenceDirectory 'deepseek-default-guard.stderr.log') -Raw)
 if($exit -ne 101 -or $err -cnotmatch '(?m)^error: target `nova_scheduler_api_real` in package `nova` requires the features: `scheduler-validation`\r?$' -or $err -match 'could not compile|error\[E[0-9]+\]'){throw 'DEEPSEEK_DEFAULT_GUARD'}
 $exit=Invoke-Fixed 'deepseek-release-guard' $cargo @('build','--offline','--locked','--release','--features','scheduler-validation','--bin','nova_scheduler_api_real','--message-format=json')
 $errors=@(Get-Content (Join-Path $EvidenceDirectory 'deepseek-release-guard.stdout.log')|ForEach-Object {if($_.StartsWith('{')){$j=$_|ConvertFrom-Json;if($j.reason -ceq 'compiler-message' -and $j.message.level -ceq 'error'){$j}}})
 if($exit -ne 101 -or $errors.Count -ne 1 -or $errors[0].target.name -cne 'nova_lib' -or $errors[0].message.message -cne 'scheduler-validation is debug-only and must never ship' -or $null -ne $errors[0].message.code){throw 'DEEPSEEK_RELEASE_GUARD'}
 $realBin=Join-Path $TargetDirectory 'debug/nova_scheduler_api_real.exe'
 foreach($case in @(@('deepseek-default-off',@()),@('deepseek-no-enable',@('run','unused')))){
  $exit=Invoke-Fixed $case[0] $realBin $case[1]
  $output=Output-Json $case[0]
  if($exit -ne 2 -or $output.Count -ne 2 -or $output.error_code -cne 'REAL_API_NOT_AUTHORIZED' -or $output.mode -cne 'REAL_API'){throw 'DEEPSEEK_DEFAULT_OFF'}
 }
 $deepseekEntry=@{debug_built=$true;default_guard='REQUIRED_FEATURES';release_guard='scheduler-validation is debug-only and must never ship';default_off_error='REAL_API_NOT_AUTHORIZED';no_enable_error='REAL_API_NOT_AUTHORIZED';network_executed=$false;real_execution_authorized=$false}
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
 Assert-Reopened $v (Output-Json 'api-reopen-status') $again
 if(($again.facts|ConvertTo-Json -Depth 30 -Compress) -cne ($v.facts|ConvertTo-Json -Depth 30 -Compress)){throw 'API_REPLAY'}
 if((Get-FileHash (Join-Path $run 'fixture/pilot/src/normalize_label.rs') -Algorithm SHA256).Hash.ToLowerInvariant() -cne $baseHash){throw 'CANDIDATE_APPLIED'}
 $handoff=Join-Path $TargetDirectory 'debug/nova_scheduler_handoff.exe';$relay=Join-Path $TargetDirectory 'debug/nova_scheduler_relay.exe'
 $before=Get-DbIdentity $workspace
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
 if((Get-DbIdentity $workspace) -cne $before){throw 'LEGACY_V3_WROTE'}
 if((Test-Path (Join-Path $EvidenceDirectory 'never-relay')) -or (Test-Path (Join-Path $EvidenceDirectory 'never-run'))){throw 'LEGACY_ROOT_CREATED'}
 $verdict='PASS';$failure='NONE'
} catch { $failure=if($_.Exception.Message -cmatch '^[A-Z0-9_]+$'){$_.Exception.Message}else{'LOCAL_CHECK_FAILED'};throw
} finally {
 # Reconstruct only closed fields. Raw logs stay private, never in the upload manifest.
 $identity=@{sha=(& $git -C $repo rev-parse HEAD).Trim();run_id=[string]$env:GITHUB_RUN_ID;attempt=[string]$env:GITHUB_RUN_ATTEMPT}
 $fixed=@{identity=$identity;schema='nova.scheduler.api-validation/1';mode='API_INJECTED_ONLY';source='ACTUAL_LOCAL_INJECTED_VALIDATION';source_sha=(& $git -C $repo rev-parse HEAD);verdict=$verdict;failure_code=$failure;network_executed=$false;real_execution_authorized=$false;github_verified=$false}
 $fakeIdentity=if(Test-Path (Join-Path $EvidenceDirectory 'old-relay/fake-cli-identity.json')){Get-Content (Join-Path $EvidenceDirectory 'old-relay/fake-cli-identity.json') -Raw|ConvertFrom-Json -AsHashtable}else{$null}
 $oldRun=if(Test-Path (Join-Path $EvidenceDirectory 'old-relay/relay-run.stdout.log')){Get-Content (Join-Path $EvidenceDirectory 'old-relay/relay-run.stdout.log') -Raw|ConvertFrom-Json -AsHashtable}else{$null}
 $checks=@{deepseek_entry=$deepseekEntry;identity=$identity;schema='nova.scheduler.api-checks/1';commands=@($rows|ForEach-Object {@{name=$_.name;state=$_.state;exit_code=$_.exit_code}});old_default_regression_required=$true;old_commands=$prior.commands;default_executed=$sh1.default_regression_executed;scheduler=$scheduler;explicit=$explicit;required_api_tests=$RequiredApiTests;reopen_equal=($verdict -ceq 'PASS');db_unchanged=($verdict -ceq 'PASS');legacy_roots_absent=($verdict -ceq 'PASS');gate_checks_verified=($verdict -ceq 'PASS');sh1_commands=$sh1.commands;fake_identity=@{sha256=$fakeIdentity.sha256;bytes=$fakeIdentity.bytes};cli_roles=@($oldRun.facts|ForEach-Object {@{role=$_.role;exe_sha256=$_.exe_sha256;state=$_.state;reclaim=$_.reclaim}})}
 $names=@{identity=$identity;schema='nova.scheduler.api-tests/1';source='ACTUAL_LOCAL_TEST_OUTPUT';listed=$tests.listed;passed=$tests.passed;ignored=$tests.ignored}
 foreach($pair in @(@('result.json',$fixed,65536),@('checks.json',$checks,131072),@('tests.json',$names,262144))){$path=Join-Path $EvidenceDirectory $pair[0];Write-Json $path $pair[1];if((Get-Item $path).Length -gt $pair[2]){throw 'SUMMARY_BOUND'}}
 $files=@('result.json','checks.json','tests.json')|ForEach-Object {$p=Join-Path $EvidenceDirectory $_;@{name=$_;bytes=(Get-Item $p).Length;sha256=(Get-FileHash $p -Algorithm SHA256).Hash.ToLowerInvariant()}}
 Write-Json (Join-Path $EvidenceDirectory 'files.json') @{schema='nova.scheduler.api-files/1';identity=$identity;files=@($files)}
 if((Get-Item (Join-Path $EvidenceDirectory 'files.json')).Length -gt 16384){throw 'MANIFEST_BOUND'}
}
if($verdict -cne 'PASS'){exit 1}
