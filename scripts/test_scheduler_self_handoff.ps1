#requires -Version 7.0
param([Parameter(Mandatory)][string]$EvidenceDirectory,[string]$TargetDirectory,[switch]$SkipTests,[switch]$UseBuiltBinary)
$ErrorActionPreference='Stop'
$repo=Split-Path -Parent $PSScriptRoot
if(-not [IO.Path]::IsPathRooted($EvidenceDirectory) -or (Test-Path -LiteralPath $EvidenceDirectory)){throw 'NEW_ABSOLUTE_EVIDENCE_REQUIRED'}
[void](New-Item -ItemType Directory -Path $EvidenceDirectory)
$crate=Join-Path $repo 'apps/desktop/src-tauri'
if(-not $TargetDirectory){$TargetDirectory=Join-Path $crate 'target'}
if(-not [IO.Path]::IsPathRooted($TargetDirectory)){throw 'ABSOLUTE_TARGET_REQUIRED'}
$cargo=(Get-Command cargo -CommandType Application|Select-Object -First 1).Source
$rows=[Collections.Generic.List[object]]::new()
function Fixed([string]$Name,[string]$Exe,[string[]]$Vector){
 $info=[Diagnostics.ProcessStartInfo]::new($Exe);$info.WorkingDirectory=$crate;$info.UseShellExecute=$false;$info.CreateNoWindow=$true;$info.RedirectStandardOutput=$true;$info.RedirectStandardError=$true;$info.Environment['CARGO_TARGET_DIR']=$TargetDirectory
 foreach($arg in $Vector){$info.ArgumentList.Add($arg)}
 $p=[Diagnostics.Process]::new();$p.StartInfo=$info
 if(-not $p.Start()){throw 'START_FAILED'}
 $o=$p.StandardOutput.ReadToEndAsync();$e=$p.StandardError.ReadToEndAsync()
 if(-not $p.WaitForExit(600000)){$p.Kill($true);throw 'SELF1_VALIDATION_TIMEOUT'}
 [IO.File]::WriteAllText((Join-Path $EvidenceDirectory "$Name.stdout.log"),$o.GetAwaiter().GetResult(),[Text.UTF8Encoding]::new($false))
 [IO.File]::WriteAllText((Join-Path $EvidenceDirectory "$Name.stderr.log"),$e.GetAwaiter().GetResult(),[Text.UTF8Encoding]::new($false))
 $code=$p.ExitCode;$p.Dispose();$rows.Add(@{name=$Name;exit_code=$code});return $code
}
function ArgsFor([string]$Verb){return @($Verb,'--offline','--locked')}
if(-not $SkipTests){if((Fixed 'targeted' $cargo @('test','--offline','--locked','--lib','scheduler::self_handoff::tests','--','--test-threads=1')) -ne 0){throw 'SELF1_TARGETED_FAILED'}}
if(-not $UseBuiltBinary){if((Fixed 'debug-build' $cargo @('build','--offline','--locked','--features','scheduler-validation','--bin','nova_scheduler_self_handoff')) -ne 0){throw 'SELF1_DEBUG_FAILED'}}
$bin=Join-Path $TargetDirectory 'debug/nova_scheduler_self_handoff.exe'
if((Fixed 'default-closed' $bin @()) -ne 2){throw 'SELF1_DEFAULT_NOT_CLOSED'}
$v=Get-Content (Join-Path $EvidenceDirectory 'default-closed.stdout.log') -Raw|ConvertFrom-Json
if($v.error_code -cne 'SELF_COMMAND_REJECTED' -or $v.real_codex_started -isnot [bool] -or $v.real_codex_started -or $v.execution_authorized){throw 'SELF1_DEFAULT_REASON'}
if((Fixed 'default-feature' $cargo @('build','--offline','--locked','--bin','nova_scheduler_self_handoff')) -ne 101){throw 'SELF1_FEATURE_GUARD'}
$err=Get-Content (Join-Path $EvidenceDirectory 'default-feature.stderr.log') -Raw
if($err -cnotmatch '(?m)^error: target `nova_scheduler_self_handoff` in package `nova` requires the features: `scheduler-validation`\r?$' -or $err -match 'error\[E\d+\]|could not compile'){throw 'SELF1_FEATURE_REASON'}
if((Fixed 'release-feature' $cargo @('build','--offline','--locked','--release','--features','scheduler-validation','--bin','nova_scheduler_self_handoff','--message-format=json')) -ne 101){throw 'SELF1_RELEASE_GUARD'}
$errors=@(Get-Content (Join-Path $EvidenceDirectory 'release-feature.stdout.log')|ForEach-Object{if($_.StartsWith('{')){$_|ConvertFrom-Json}}|Where-Object{$_.reason -ceq 'compiler-message' -and $_.message.level -ceq 'error'})
if($errors.Count -ne 1 -or $errors[0].target.name -cne 'nova_lib' -or $errors[0].message.message -cne 'scheduler-validation is debug-only and must never ship' -or $null -ne $errors[0].message.code){throw 'SELF1_RELEASE_REASON'}
[IO.File]::WriteAllText((Join-Path $EvidenceDirectory 'result.json'),(@{schema='nova.self1-entry-validation/1';commands=@($rows);tests=if($SkipTests){'REUSED_PARENT_FULL_EXECUTION'}else{'ACTUAL_TARGETED'};real_codex_started=$false;network_executed=$false;verdict='PASS'}|ConvertTo-Json -Depth 8),[Text.UTF8Encoding]::new($false))
