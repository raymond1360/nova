#requires -Version 7.0
param([Parameter(Mandatory)][string]$EvidenceDirectory,[string]$TargetDirectory,[switch]$SkipTests,[switch]$UseBuiltBinary)
$ErrorActionPreference='Stop'
$repo=Split-Path -Parent $PSScriptRoot
$crate=Join-Path $repo 'apps/desktop/src-tauri'
if(-not [IO.Path]::IsPathRooted($EvidenceDirectory) -or (Test-Path -LiteralPath $EvidenceDirectory)){throw 'NEW_ABSOLUTE_EVIDENCE_REQUIRED'}
[void](New-Item -ItemType Directory -Path $EvidenceDirectory)
if(-not $TargetDirectory){$TargetDirectory=Join-Path $crate 'target'}
$cargo=(Get-Command cargo -CommandType Application|Select-Object -First 1).Source
$git=(Get-Command git -CommandType Application|Select-Object -First 1).Source
$rows=[Collections.Generic.List[object]]::new()
function Write-Json([string]$p,$v){[IO.File]::WriteAllText($p,($v|ConvertTo-Json -Depth 40),[Text.UTF8Encoding]::new($false))}
function Fixed([string]$Name,[string]$Exe,[string[]]$Vector,[string]$Cwd=$crate){
 $info=[Diagnostics.ProcessStartInfo]::new($Exe);$info.WorkingDirectory=$Cwd;$info.UseShellExecute=$false;$info.CreateNoWindow=$true;$info.RedirectStandardOutput=$true;$info.RedirectStandardError=$true;$info.Environment['CARGO_TARGET_DIR']=$TargetDirectory;$info.Environment['CARGO_NET_OFFLINE']='true';$info.Environment['RUSTUP_AUTO_INSTALL']='0'
 foreach($a in $Vector){$info.ArgumentList.Add($a)}
 $p=[Diagnostics.Process]::new();$p.StartInfo=$info
 if(-not $p.Start()){throw 'START_FAILED'}
 $o=$p.StandardOutput.ReadToEndAsync();$e=$p.StandardError.ReadToEndAsync()
 if(-not $p.WaitForExit(600000)){$p.Kill($true);throw 'SELF2_VALIDATION_TIMEOUT'}
 [IO.File]::WriteAllText((Join-Path $EvidenceDirectory "$Name.stdout.log"),$o.GetAwaiter().GetResult(),[Text.UTF8Encoding]::new($false))
 [IO.File]::WriteAllText((Join-Path $EvidenceDirectory "$Name.stderr.log"),$e.GetAwaiter().GetResult(),[Text.UTF8Encoding]::new($false))
 $code=$p.ExitCode;$p.Dispose();$rows.Add(@{name=$Name;exe=$Exe;argv=$Vector;exit_code=$code});Write-Json (Join-Path $EvidenceDirectory 'commands.json') @($rows);return $code
}
function Zero([string]$n,[string]$exe,[string[]]$a,[string]$cwd=$crate){if((Fixed $n $exe $a $cwd) -ne 0){throw "SELF2_STAGE_FAILED:$n"}}
if(-not $SkipTests){Zero 'targeted' $cargo @('test','--offline','--locked','--lib','scheduler::self_codex::tests','--','--test-threads=1')}
if(-not $UseBuiltBinary){Zero 'debug-build' $cargo @('build','--offline','--locked','--features','scheduler-validation','--bin','nova_scheduler_self_codex','--bin','nova_scheduler_self2_fake_cli')}
$bin=Join-Path $TargetDirectory 'debug/nova_scheduler_self_codex.exe'
$fake=Join-Path $TargetDirectory 'debug/nova_scheduler_self2_fake_cli.exe'
foreach($case in @(@('default-closed',@(),'SELF2_COMMAND_REJECTED'),@('native-admission',@('native-run'),'SELF2_NATIVE_PERMISSION_AND_UUID_ADMISSION_REQUIRED'),@('permission-nonconsole',@('permission-run','W',('0'*64)),'CONSOLE_REQUIRED'),@('permission-helper-scope',@('permission-helper','outside','invalid'),'PROBE_PHASE_REJECTED'))){
 if((Fixed $case[0] $bin $case[1]) -ne 2){throw 'SELF2_NOT_CLOSED'}
 $v=Get-Content (Join-Path $EvidenceDirectory "$($case[0]).stdout.log") -Raw|ConvertFrom-Json
 if($v.error_code -cne $case[2] -or $v.real_codex_started -or $v.network_executed){throw 'SELF2_REASON_REJECTED'}
}
foreach($name in @('nova_scheduler_self_codex','nova_scheduler_self2_fake_cli')){
 if((Fixed "feature-$name" $cargo @('build','--offline','--locked','--bin',$name)) -ne 101){throw 'SELF2_FEATURE_GUARD'}
 $err=Get-Content (Join-Path $EvidenceDirectory "feature-$name.stderr.log") -Raw
 if($err -cnotmatch ('(?m)^error: target `'+[regex]::Escape($name)+'` in package `nova` requires the features: `scheduler-validation`\r?$') -or $err -match 'error\[E\d+\]|could not compile'){throw 'SELF2_FEATURE_REASON'}
}
if((Fixed 'release' $cargo @('build','--offline','--locked','--release','--features','scheduler-validation','--bin','nova_scheduler_self_codex','--message-format=json')) -ne 101){throw 'SELF2_RELEASE_GUARD'}
$errors=@(Get-Content (Join-Path $EvidenceDirectory 'release.stdout.log')|ForEach-Object{if($_.StartsWith('{')){$_|ConvertFrom-Json}}|Where-Object{$_.reason -ceq 'compiler-message' -and $_.message.level -ceq 'error'})
if($errors.Count -ne 1 -or $errors[0].target.name -cne 'nova_lib' -or $errors[0].message.message -cne 'scheduler-validation is debug-only and must never ship' -or $null -ne $errors[0].message.code){throw 'SELF2_RELEASE_REASON'}
# A new owned synthetic project/store, never the application's daily database or an existing chat.
$fixture=Join-Path $EvidenceDirectory 'fixture';[void](New-Item -ItemType Directory -Path $fixture)
$project=Join-Path $fixture 'repo';$reports=Join-Path $fixture 'reports';[void](New-Item -ItemType Directory -Path $project);[void](New-Item -ItemType Directory -Path $reports)
Zero 'git-init' $git @('init','-q') $project
[IO.File]::WriteAllText((Join-Path $project 'a.txt'),"SYNTHETIC transport fixture`n",[Text.UTF8Encoding]::new($false))
Zero 'git-add' $git @('add','a.txt') $project
Zero 'git-commit' $git @('-c','user.name=SELF2 fixture','-c','user.email=fixture@invalid','-c','core.hooksPath=NUL','commit','-qm','offline fixture') $project
Zero 'git-head' $git @('rev-parse','HEAD') $project
$head=(Get-Content (Join-Path $EvidenceDirectory 'git-head.stdout.log') -Raw).Trim()
$report=@{schema='nova.scheduler.report/1';report_type='DEVELOPMENT';claimed_delivery_sha=$head;claimed_conclusion='NOT_RUN';issues=@();context=@{package_id='self2-fixture';stage='INITIAL';supersedes_sha=$null;pm_decision_ref=$null;decision_kind=$null;slot_resolution=$null};claims=@{author='SYNTHETIC fixture';role='development';fresh_session=$null;model=$null;ci_runs=@()};source_report=$null;transcribed_by=$null}
$delivery=@{stage='self2-stage';package='self2-fixture';base_sha=$head;head_sha=$head;goal='SYNTHETIC native transport exercise, no product development';acceptance=@('No project edits; supervised fake CLI relay only');allowed_paths=@('a.txt');required_commands=@(,@($fake,'--nova-self2-synthetic-only','--fixed-check'));logs=@();developer_report=$report}
$stage=@{stage='self2-stage';developer=$null;reviewer=$null}
$plan=@{mode='Synthetic';schema='nova.self2.plan/1';input=$delivery;project_root=$project;cli='C:/synthetic/codex.exe';cli_sha256='dd13bdb162a184fa04039488b234a8258ef9b76f84e926c505d5742b19249a6a';cli_version='codex-cli 0.162.1';model='SYNTHETIC_ONLY';commands=@(@{exe=$fake;sha256=(Get-FileHash -LiteralPath $fake -Algorithm SHA256).Hash.ToLowerInvariant();args=@('--nova-self2-synthetic-only','--fixed-check')})}
Write-Json (Join-Path $fixture 'input.json') $delivery;Write-Json (Join-Path $fixture 'stage.json') $stage;Write-Json (Join-Path $fixture 'plan.json') $plan
$self1=Join-Path $TargetDirectory 'debug/nova_scheduler_self_handoff.exe'
Zero 'store-path' $self1 @('store-path',$project)
$store=(Get-Content (Join-Path $EvidenceDirectory 'store-path.stdout.log') -Raw|ConvertFrom-Json).canonical_store
Zero 'init-work' $bin @('init-work',$store,$project,$reports,(Join-Path $fixture 'stage.json'),(Join-Path $fixture 'input.json'),(Join-Path $fixture 'plan.json'))
Zero 'fake-relay' $bin @('fake-run',$store,$fake,$plan.commands[0].sha256)
Zero 'cold-status' $bin @('status',$store)
Zero 'no-replay' $bin @('fake-run',$store,$fake,$plan.commands[0].sha256)
Zero 'export-packet' $bin @('export',$store,(Join-Path $fixture 'review-packet'))
$v=Get-Content (Join-Path $EvidenceDirectory 'fake-relay.stdout.log') -Raw|ConvertFrom-Json
$reopen=Get-Content (Join-Path $EvidenceDirectory 'no-replay.stdout.log') -Raw|ConvertFrom-Json
if($v.real_codex_started -or $v.self1.active.status -cne 'AWAITING_PM_AND_USER' -or $v.journal.records.Count -ne 3 -or $reopen.journal.records.Count -ne 3){throw 'SELF2_DEMO_REJECTED'}
foreach($r in $v.journal.records){if($r.outcome -cne 'COMPLETE' -or $r.attempt.exit_code -ne 0 -or $r.execution.reclaim -cne 'CONFIRMED' -or -not $r.attempt.pid -or -not $r.attempt.process_creation_utc){throw 'SELF2_RECEIPT_REJECTED'}}
if($v.self1.stage.developer.uuid -ceq $v.self1.stage.reviewer.uuid -or $v.self1.stage.reviewer.source -cne 'SYNTHETIC_CLI_RECEIPT'){throw 'SELF2_BINDING_REJECTED'}
Write-Json (Join-Path $EvidenceDirectory 'result.json') @{schema='nova.self2-entry-validation/1';verdict='PASS';commands=@($rows);tests=if($SkipTests){'REUSED_PARENT_FULL_EXECUTION'}else{'ACTUAL_TARGETED'};native_fake_cli=$true;native_codex='NOT RUN';real_codex_started=$false;network_executed=$false;bin_sha256=(Get-FileHash $bin -Algorithm SHA256).Hash.ToLowerInvariant();fake_sha256=$plan.commands[0].sha256;packet=(Join-Path $fixture 'review-packet')}
