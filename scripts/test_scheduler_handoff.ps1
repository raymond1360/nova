#requires -Version 7.0
param([Parameter(Mandatory)][string]$EvidenceDirectory,[string]$TargetDirectory,[switch]$SkipDefaultRegression)
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
$clock=[Diagnostics.Stopwatch]::StartNew();$deadline=2700000
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
try {
 Require-Zero 'debug-build' $cargo @('build','--offline','--locked','--features','scheduler-validation','--bin','nova_scheduler_handoff')
 $bin=Join-Path $TargetDirectory 'debug/nova_scheduler_handoff.exe'
 $stage='SCHEDULER_TESTS'
 Require-Zero 'scheduler-list' $cargo @('test','--offline','--locked','--lib','scheduler::','--','--list')
 Require-Zero 'scheduler-tests' $cargo @('test','--offline','--locked','--lib','scheduler::','--','--test-threads=1')
 Require-Zero 'adapter-tests' $cargo @('test','--offline','--locked','--features','scheduler-validation','--bin','nova_scheduler_handoff','--','--test-threads=1')
 $stage='DEFAULT_REQUIRED_FEATURES'
 $code=Invoke-Fixed 'default-required-features' $cargo @('build','--offline','--locked','--bin','nova_scheduler_handoff')
 $err=[string](Get-Content (Join-Path $EvidenceDirectory 'default-required-features.stderr.log') -Raw)
 if($code -ne 101 -or $err -notmatch '(?m)^error: target `nova_scheduler_handoff` in package `nova` requires the features: `scheduler-validation`\r?$' -or $err -match 'could not compile'){throw 'Not exact required-features rejection'}
 $stage='RELEASE_GUARD'
 $code=Invoke-Fixed 'release-guard' $cargo @('check','--offline','--locked','--release','--features','scheduler-validation','--bin','nova_scheduler_handoff','--message-format=json')
 $errors=@(Get-Content (Join-Path $EvidenceDirectory 'release-guard.stdout.log') | ForEach-Object {if($_.StartsWith('{')){$v=ConvertFrom-Json $_;if($v.reason -eq 'compiler-message' -and $v.message.level -eq 'error'){$v}}})
 if($code -ne 101 -or $errors.Count -ne 1 -or $errors[0].target.name -cne 'nova_lib' -or $errors[0].message.message -cne 'scheduler-validation is debug-only and must never ship'){throw 'Not exact scheduler guard'}
 $stage='FOUR_COMMAND_FIXTURE'
 $fixture=Join-Path $EvidenceDirectory 'git-fixture';$reports=Join-Path $EvidenceDirectory 'reports';$owned=Join-Path $EvidenceDirectory 'owned'
 $hooks=Join-Path $EvidenceDirectory 'empty-hooks';$template=Join-Path $EvidenceDirectory 'empty-template';$empty=Join-Path $EvidenceDirectory 'empty-gitconfig'
 foreach($path in @($fixture,$reports,$hooks,$template)){New-Item -ItemType Directory -Path $path | Out-Null};[IO.File]::WriteAllText($empty,'')
 $prefix=@('--no-pager','--no-optional-locks','-c',"core.hooksPath=$hooks",'-c','commit.gpgSign=false','-c','tag.gpgSign=false','-c','core.fsmonitor=false','-c','core.autocrlf=false','-c','core.pager=cat','-c','color.ui=false')
 Require-Zero 'fixture-init' $git ($prefix+@('init',"--template=$template",'--initial-branch=fixture',$fixture)) $EvidenceDirectory -FixtureGit
 [IO.File]::WriteAllText((Join-Path $fixture 'Logo.tsx'),"export const Logo = 'before';`n",[Text.UTF8Encoding]::new($false))
 [IO.File]::WriteAllText((Join-Path $fixture 'scope-extra.txt'),"fixed extra`n",[Text.UTF8Encoding]::new($false))
 Require-Zero 'fixture-add-base' $git ($prefix+@('-C',$fixture,'add','--','Logo.tsx','scope-extra.txt')) $EvidenceDirectory -FixtureGit
 Require-Zero 'fixture-commit-base' $git ($prefix+@('-C',$fixture,'commit','--no-gpg-sign','--no-verify','-m','fixture-base')) $EvidenceDirectory -FixtureGit
 Require-Zero 'fixture-base-sha' $git ($prefix+@('-C',$fixture,'rev-parse','--verify','HEAD^{commit}')) $EvidenceDirectory -FixtureGit
 $base=([string](Get-Content (Join-Path $EvidenceDirectory 'fixture-base-sha.stdout.log') -Raw)).Trim()

 $rootManifest=Join-Path $EvidenceDirectory 'root-commit-manifest.json'
 Write-Json $rootManifest @{schema='nova.sh1.package/1';package_id='root-fixture';repo_path=$fixture;title='Root unsupported';goal='Report fixed limitation';allowed_paths=@('Logo.tsx');acceptance_items=@('explicit error');pm_decision_ref='fixture-only';report_roots=@($reports);required_reports=@('DEVELOPMENT','REVIEW','CI');baseline=@{mode='PRE_WORK';base_sha=$null};eol_policy='false'}
 if((Invoke-Fixed 'root-commit-rejected' $bin @('init','--workspace',(Join-Path $EvidenceDirectory 'owned-root-rejected'),'--manifest',$rootManifest)) -ne 2 -or (Output-Json 'root-commit-rejected').error_code -cne 'ROOT_COMMIT_UNSUPPORTED'){throw 'Root limitation not explicit'}
 [IO.File]::WriteAllText((Join-Path $fixture 'Logo.tsx'),"export const Logo = 'after';`n",[Text.UTF8Encoding]::new($false))
 Require-Zero 'fixture-add-change' $git ($prefix+@('-C',$fixture,'add','--','Logo.tsx')) $EvidenceDirectory -FixtureGit
 Require-Zero 'fixture-commit-change' $git ($prefix+@('-C',$fixture,'commit','--no-gpg-sign','--no-verify','-m','fixture-change-1')) $EvidenceDirectory -FixtureGit
 $manifest=Join-Path $EvidenceDirectory 'manifest.json'
 Write-Json $manifest @{schema='nova.sh1.package/1';package_id='sh1-fixed-fixture';repo_path=$fixture;title='Handoff fixture';goal='Record one bounded edit';allowed_paths=@('Logo.tsx');acceptance_items=@('fixed fixture');pm_decision_ref='fixture-only';report_roots=@($reports);required_reports=@('DEVELOPMENT','REVIEW','CI');baseline=@{mode='DECLARED';base_sha=$base};eol_policy='false'}
 Require-Zero 'bin-init' $bin @('init','--workspace',$owned,'--manifest',$manifest)
 $initial=Output-Json 'bin-init';$head=$initial.head.value
 $nestedManifest=Get-Content -LiteralPath $manifest -Raw | ConvertFrom-Json -AsHashtable
 $nestedManifest.allowed_paths=@('brand/new/Logo.tsx');$nestedManifest.baseline=@{mode='PRE_WORK';base_sha=$null}
 $nestedPath=Join-Path $EvidenceDirectory 'manifest-absent-nested.json';Write-Json $nestedPath $nestedManifest
 Require-Zero 'absent-nested-init' $bin @('init','--workspace',(Join-Path $EvidenceDirectory 'owned-absent-nested'),'--manifest',$nestedPath)
 if(Test-Path -LiteralPath (Join-Path $fixture 'brand')){throw 'ABSENT_CREATED_USER_DIRECTORY'}

 if((Invoke-Fixed 'handoff-missing-slots' $bin @('handoff','--workspace',$owned)) -ne 2){throw 'Incomplete handoff was not held'}
 $legacy=Join-Path $reports 'legacy.md';[IO.File]::WriteAllText($legacy,'historical human report, no fixed schema')
 Require-Zero 'legacy-index-only' $bin @('import','--workspace',$owned,'--report',$legacy,'--index-only')
 $legacyId=(Output-Json 'legacy-index-only').import.receipt_id
 $roles=@{DEVELOPMENT='development';REVIEW='review';CI='ci'}
 foreach($type in @('REVIEW','CI','DEVELOPMENT')){
  $r=@{schema='nova.scheduler.report/1';report_type=$type;claimed_delivery_sha=$head;claimed_conclusion='PASS';issues=@();context=@{package_id='sh1-fixed-fixture';stage='INITIAL';supersedes_sha=$null;pm_decision_ref=$null;decision_kind=$null;slot_resolution=$null};claims=@{author='Fixture human claim';role=$roles[$type];fresh_session=$null;model=$null;ci_runs=@()};source_report=$null;transcribed_by=$null}
  $file=Join-Path $reports "$type.json";Write-Json $file $r;Require-Zero "bin-import-$type" $bin @('import','--workspace',$owned,'--report',$file)
 }
 Require-Zero 'bin-status' $bin @('status','--workspace',$owned)
 Require-Zero 'bin-handoff' $bin @('handoff','--workspace',$owned)
 $v=Output-Json 'bin-handoff';if($v.phase -cne 'AWAITING_PM' -or $v.projection.complete -ne $true -or $v.execution_permission -cne 'NOT_GRANTED_BY_SH1'){throw 'Handoff contract failed'}
 if(@($v.receipts | Where-Object {$_.receipt_id -ceq $legacyId -and $_.mode -ceq 'INDEX_ONLY'}).Count -ne 1){throw 'Index receipt ID not displayed'}
 Require-Zero 'bin-repeat' $bin @('import','--workspace',$owned,'--report',(Join-Path $reports 'REVIEW.json'))
 if((Output-Json 'bin-repeat').import.idempotent -ne $true){throw 'Repeat not idempotent'}
 [IO.File]::WriteAllText((Join-Path $fixture 'unrelated.txt'),'outside notice')
 Require-Zero 'outside-untracked' $bin @('status','--workspace',$owned)
 if((Output-Json 'outside-untracked').delivery_identity -cne $v.delivery_identity){throw 'Outside file invalidated delivery'}
 [IO.File]::WriteAllText((Join-Path $fixture '.gitattributes'),"unrelated.txt filter=fixture-never-execute`n",[Text.UTF8Encoding]::new($false))
 Require-Zero 'outside-filter' $bin @('status','--workspace',$owned)
 if((Output-Json 'outside-filter').outside_scope_notice -cne 'UNKNOWN:UNSUPPORTED_FILTER'){throw 'Outside filter was not suppressed'}
 [IO.File]::WriteAllText((Join-Path $fixture '.gitattributes'),"Logo.tsx filter=fixture-never-execute`n",[Text.UTF8Encoding]::new($false))
 if((Invoke-Fixed 'scope-filter' $bin @('status','--workspace',$owned)) -ne 2 -or (Output-Json 'scope-filter').error_code -cne 'UNSUPPORTED_FILTER'){throw 'Scope filter not rejected'}
 [IO.File]::WriteAllText((Join-Path $fixture '.gitattributes'),"Logo.tsx filter`n",[Text.UTF8Encoding]::new($false))
 if((Invoke-Fixed 'scope-filter-set' $bin @('status','--workspace',$owned)) -ne 2 -or (Output-Json 'scope-filter-set').error_code -cne 'UNSUPPORTED_FILTER'){throw 'Set filter not rejected'}
 [IO.File]::WriteAllText((Join-Path $fixture '.gitattributes'),'')
 [IO.File]::WriteAllText((Join-Path $fixture 'Logo.tsx'),"dirty`n",[Text.UTF8Encoding]::new($false))
 if((Invoke-Fixed 'scope-dirty-repeat' $bin @('import','--workspace',$owned,'--report',(Join-Path $reports 'REVIEW.json'))) -ne 2 -or (Output-Json 'scope-dirty-repeat').error_code -cne 'DIRTY_SCOPE'){throw 'Dirty scope bypassed idempotency'}
 $stage='EOL_AND_PATH_FIXTURES'
 [IO.File]::WriteAllText((Join-Path $fixture 'Logo.tsx'),"export const Logo = 'after';`r`n",[Text.UTF8Encoding]::new($false))
 if((Invoke-Fixed 'crlf-false-dirty' $bin @('status','--workspace',$owned)) -ne 2 -or (Output-Json 'crlf-false-dirty').error_code -cne 'DIRTY_SCOPE'){throw 'False EOL policy not enforced'}
 $manifestValue=Get-Content -LiteralPath $manifest -Raw | ConvertFrom-Json -AsHashtable
 foreach($eol in @('true','input')){
  $manifestValue.eol_policy=$eol;$mp=Join-Path $EvidenceDirectory "manifest-$eol.json";Write-Json $mp $manifestValue
  $storePath=Join-Path $EvidenceDirectory "owned-$eol"
  Require-Zero "crlf-init-$eol" $bin @('init','--workspace',$storePath,'--manifest',$mp)
  Require-Zero "crlf-status-$eol" $bin @('status','--workspace',$storePath)
 }
 [IO.File]::WriteAllText((Join-Path $fixture '.gitattributes'),"Logo.tsx text eol=crlf`n",[Text.UTF8Encoding]::new($false))
 Require-Zero 'explicit-eol-crlf' $bin @('status','--workspace',$owned)
 [IO.File]::WriteAllText((Join-Path $fixture '.gitattributes'),"Logo.tsx -text eol=crlf`n",[Text.UTF8Encoding]::new($false))
 if((Invoke-Fixed 'contradictory-attributes' $bin @('status','--workspace',$owned)) -ne 2 -or (Output-Json 'contradictory-attributes').error_code -cne 'UNSUPPORTED_ATTRIBUTES'){throw 'Contradictory attributes not rejected'}
 [IO.File]::WriteAllText((Join-Path $fixture '.gitattributes'),'')
 [IO.File]::WriteAllText((Join-Path $fixture 'Logo.tsx'),"export const Logo = 'after';`n",[Text.UTF8Encoding]::new($false))
 $junction=Join-Path $EvidenceDirectory 'fixture-alias'
 New-Item -ItemType Junction -Path $junction -Target $fixture | Out-Null
 $manifestValue.eol_policy='false';$manifestValue.repo_path=$junction
 $mp=Join-Path $EvidenceDirectory 'manifest-alias.json';Write-Json $mp $manifestValue
 if((Invoke-Fixed 'junction-rejected' $bin @('init','--workspace',(Join-Path $EvidenceDirectory 'alias-owned'),'--manifest',$mp)) -ne 2 -or (Output-Json 'junction-rejected').error_code -cne 'LINK_REJECTED'){throw 'Junction not rejected'}
 if((Invoke-Fixed 'unknown-command' $bin @('--offline-probe-once')) -ne 2 -or (Output-Json 'unknown-command').error_code -cne 'ARGUMENT_REJECTED'){throw 'Unexpected entry reachable'}
 [IO.File]::WriteAllText((Join-Path $fixture 'Logo.tsx'),"staged edit`n",[Text.UTF8Encoding]::new($false))
 Require-Zero 'fixture-add-staged' $git ($prefix+@('-C',$fixture,'add','--','Logo.tsx')) $EvidenceDirectory -FixtureGit
 if((Invoke-Fixed 'staged-scope' $bin @('status','--workspace',$owned)) -ne 2 -or (Output-Json 'staged-scope').error_code -cne 'DIRTY_SCOPE'){throw 'Staged scope accepted'}
 [IO.File]::WriteAllText((Join-Path $fixture 'scope-extra.txt'),"outside cumulative change`n",[Text.UTF8Encoding]::new($false))
 Require-Zero 'fixture-add-extra' $git ($prefix+@('-C',$fixture,'add','--','scope-extra.txt')) $EvidenceDirectory -FixtureGit
 Require-Zero 'fixture-commit-extra' $git ($prefix+@('-C',$fixture,'commit','--no-gpg-sign','--no-verify','-m','fixture-change-2')) $EvidenceDirectory -FixtureGit
 if((Invoke-Fixed 'cumulative-scope-rejected' $bin @('status','--workspace',$owned)) -ne 2 -or (Output-Json 'cumulative-scope-rejected').error_code -cne 'SCOPE_EXCEEDED'){throw 'Cumulative outside change missed'}
 if(-not $SkipDefaultRegression){
 $stage='DEFAULT_REGRESSION'
 Require-Zero 'default-list' $cargo @('test','--offline','--locked','--lib','--','--list')
 Require-Zero 'default-tests' $cargo @('test','--offline','--locked','--lib','--','--test-threads=1')
 Require-Zero 'default-build' $cargo @('build','--offline','--locked')
 }
 $verdict='PASS'
} finally {
 $inventory=@(Get-ChildItem -LiteralPath $EvidenceDirectory -File | Where-Object {$_.Extension -eq '.log'} | ForEach-Object {@{name=$_.Name;bytes=$_.Length;sha256=(Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()}})
 Write-Json (Join-Path $EvidenceDirectory 'result.json') @{schema='nova.scheduler.local-validation/1';verdict=$verdict;stage=$stage;default_regression_executed=(-not $SkipDefaultRegression);git_process_identity_boundary='Guarded launcher only; delegated images not frozen';commands=$rows;logs=$inventory;external_actions_executed='FIXED_LOCAL_FIXTURE_AND_CARGO_ONLY';github_verified=$false}
}
if($verdict -ne 'PASS'){exit 1}
