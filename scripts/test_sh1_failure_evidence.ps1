#requires -Version 7.0
param([Parameter(Mandatory)][string]$EvidenceDirectory)
$ErrorActionPreference='Stop'
if(-not [IO.Path]::IsPathRooted($EvidenceDirectory) -or (Test-Path -LiteralPath $EvidenceDirectory)){throw 'New evidence directory required'}
New-Item -ItemType Directory -Path $EvidenceDirectory | Out-Null
. (Join-Path $PSScriptRoot 'sh1_failure_diagnostics.ps1')
function Check($v,[string]$label){if(-not $v){throw "Synthetic assertion: $label"}}
function Put($path,$value){[IO.File]::WriteAllText($path,($value|ConvertTo-Json -Depth 20),[Text.UTF8Encoding]::new($false))}
$secret='SYNTHETIC_SECRET_NEVER_EXPORT_91d34'
$source=[IO.File]::ReadAllText((Join-Path $PSScriptRoot 'test_scheduler_handoff.ps1'))
$parseLine=($source -split "`n" | Where-Object {$_.StartsWith(' $errors=@(Get-Content')})
Check (@($parseLine).Count -eq 1) 'actual parser extracted'
$guard='scheduler-validation is debug-only and must never ship'
foreach($case in @('exact','duplicate','wrong-fields','malformed')){
 $EvidenceDirectoryCase=Join-Path $EvidenceDirectory $case;New-Item -ItemType Directory $EvidenceDirectoryCase | Out-Null
 $json=@{reason='compiler-message';target=@{name='nova_lib'};message=@{level='error';message=$guard}}
 if($case -eq 'wrong-fields'){$json.target.name=$secret;$json.message.message=$secret}
 $line=$json|ConvertTo-Json -Depth 8 -Compress
 if($case -eq 'duplicate'){$line+="`n$line"}
 if($case -eq 'malformed'){$line='{"secret":"'+$secret+'",invalid}'}
 [IO.File]::WriteAllText((Join-Path $EvidenceDirectoryCase 'release-guard.stdout.log'),$line)
 [IO.File]::WriteAllText((Join-Path $EvidenceDirectoryCase 'release-guard.stderr.log'),('Authorization: Bearer '+$secret+('x'*200000)))
 $diagnosticFacts=@{exit_is_101=$true;json_parse_complete=$false;json_line=0}
 # Execute only the actual JSON-read statement, never the SH1 script or a child command.
 $savedDirectory=$EvidenceDirectory;$EvidenceDirectory=$EvidenceDirectoryCase
 try {
  . ([scriptblock]::Create($parseLine))
  $diagnosticFacts.json_parse_complete=$true;$diagnosticFacts.error_count=$errors.Count;$diagnosticFacts.error_count_is_one=($errors.Count -eq 1)
  if($errors.Count -gt 0){$diagnosticFacts.target_is_nova_lib=($errors[0].target.name -ceq 'nova_lib');$diagnosticFacts.message_is_guard=($errors[0].message.message -ceq $guard)}
  throw 'Not exact scheduler guard'
 } catch {$detail=New-Sh1FailureDetail $_ 'RELEASE_GUARD' $diagnosticFacts $EvidenceDirectory $PSCommandPath}
 finally {$EvidenceDirectory=$savedDirectory}
 $public=$detail|ConvertTo-Json -Depth 20
 Check (-not $public.Contains($secret) -and $public.Length -lt 4096) 'bounded secret-free diagnostic'
 Check ($detail.logs.Count -eq 2) 'original log hashes retained'
 Check ($detail.logs[0].sha256 -eq (Get-FileHash (Join-Path $EvidenceDirectoryCase 'release-guard.stdout.log')).Hash.ToLowerInvariant()) 'stdout hash'
 if($case -eq 'malformed'){Check ($detail.classification -eq 'JSON_PARSE_REJECTED' -and -not $detail.facts.json_parse_complete) 'parse failure distinct'}
 else {Check ($detail.classification -eq 'RELEASE_GUARD_ASSERTION' -and $detail.line -gt 0) 'assertion location';Check ($detail.facts.error_count_is_one -eq ($case -ne 'duplicate')) 'count fact';Check ($detail.facts.target_is_nova_lib -eq ($case -ne 'wrong-fields')) 'target fact';Check ($detail.facts.message_is_guard -eq ($case -ne 'wrong-fields')) 'message fact'}
 Put (Join-Path $EvidenceDirectoryCase 'result.json') @{failure_detail=$detail}
 Check ((Read-Sh1FailureDetail (Join-Path $EvidenceDirectoryCase 'result.json')).classification -eq $detail.classification) 'reader roundtrip'
 Write-Output "PASS $case"
}
try {throw ('arbitrary '+$secret)}catch{$unknown=New-Sh1FailureDetail $_ $secret @{} $EvidenceDirectory 'not-this-script'}
Check ($unknown.classification -eq 'SCRIPT_EXCEPTION' -and $unknown.stage -eq 'UNKNOWN' -and $null -eq $unknown.line) 'unknown text and foreign location withheld'
$hostile=@{schema='nova.sh1.failure-detail/1';stage=@('BUILD')*100;classification=$secret;line=$secret;facts=@{target_is_nova_lib=$secret};logs=@(@{name=$secret;bytes=1;sha256=$secret})*100;raw=$secret}
Check ((ConvertTo-Sh1FailureDetail $hostile|ConvertTo-Json -Depth 20).Length -lt 1024) 'hostile fields bounded'
[IO.File]::WriteAllText((Join-Path $EvidenceDirectory 'oversize.json'),'x'*131073)
Check ((Read-Sh1FailureDetail (Join-Path $EvidenceDirectory 'oversize.json')).state -eq 'MISSING_OR_INVALID') 'oversize refused'
Check ((Read-Sh1FailureDetail (Join-Path $EvidenceDirectory 'absent.json')).state -eq 'MISSING_OR_INVALID') 'missing unknown'
# Execute the unchanged SH1 exporter with incomplete synthetic evidence; it must still fail.
$repo=Split-Path $PSScriptRoot -Parent;$env:GITHUB_WORKSPACE=$repo
$wf=[IO.File]::ReadAllText((Join-Path $repo '.github/workflows/windows-ci.yml'))
$begin=$wf.IndexOf('          function Test-Sh1Integer');$finish=$wf.IndexOf('      - name: Upload only reconstructed SH1 CI evidence',$begin)
Check ($begin -gt 0 -and $finish -gt $begin) 'exact exporter extraction'
$step=(($wf.Substring($begin,$finish-$begin) -split "`n")|ForEach-Object {if($_.StartsWith('          ')){$_.Substring(10)}else{$_}})-join "`n"
[IO.File]::WriteAllText((Join-Path $EvidenceDirectory 'exact-export-step.ps1'),$step)
$env:SH1_ROOT=Join-Path $EvidenceDirectory 'export';New-Item -ItemType Directory (Join-Path $env:SH1_ROOT 'evidence') | Out-Null;New-Item -ItemType Directory (Join-Path $env:SH1_ROOT 'ci-public') | Out-Null
$env:GITHUB_SHA='1111111111111111111111111111111111111111';$env:GITHUB_RUN_ID='123';$env:GITHUB_RUN_ATTEMPT='1'
Put (Join-Path $env:SH1_ROOT 'execution.json') @{state='COMPLETE';exit_code=1}
Put (Join-Path $env:SH1_ROOT 'evidence/result.json') @{failure_detail=$detail;commands=@();raw=$secret}
try {. ([scriptblock]::Create($step));throw 'Expected failure'}catch{Check ($_.Exception.Message -eq 'SH1_INPUT_MISSING') 'original rejection unchanged'}
$public=Join-Path $env:SH1_ROOT 'ci-public';$checks=Get-Content (Join-Path $public 'checks.json') -Raw | ConvertFrom-Json -AsHashtable
Check ($checks.failure_detail.classification -eq 'JSON_PARSE_REJECTED' -and $checks.state -eq 'FAIL') 'real exporter carries safe failure'
Check (@(Get-ChildItem $public -File).Count -eq 4) 'four-file structure'
$manifest=Get-Content (Join-Path $public 'files.json') -Raw|ConvertFrom-Json
foreach($row in $manifest.files){Check ((Get-FileHash (Join-Path $public $row.name)).Hash.ToLowerInvariant() -eq $row.sha256) 'manifest hashes'}
foreach($file in Get-ChildItem $public -File){Check (-not ([IO.File]::ReadAllText($file.FullName)).Contains($secret)) 'public secret absent'}
Write-Output 'PASS unknown-hostile-missing-oversize'
Write-Output 'PASS exact-sh1-exporter-failure-manifest'
Write-Output 'SH1_FAILURE_DIAGNOSTICS_SYNTHETIC_PASS'
