#requires -Version 7.0
param([Parameter(Mandatory)][string]$EvidenceDirectory)
$ErrorActionPreference='Stop'
if(-not [IO.Path]::IsPathRooted($EvidenceDirectory) -or (Test-Path -LiteralPath $EvidenceDirectory)){throw 'New absolute evidence directory required'}
New-Item -ItemType Directory -Path $EvidenceDirectory | Out-Null
$testEvidence=$EvidenceDirectory
$wf=[IO.File]::ReadAllText((Join-Path (Split-Path $PSScriptRoot -Parent) '.github/workflows/windows-ci.yml'))
$begin=$wf.IndexOf('          function Get-Sh1ExpectedCommands {',$wf.IndexOf('      - name: Verify and reconstruct bounded AR0 evidence'))
$finish=$wf.IndexOf('      - name: Upload only closed AR0 evidence',$begin)
if($begin -lt 0 -or $finish -lt $begin){throw 'Extraction failed'}
$stepText=(($wf.Substring($begin,$finish-$begin) -split "`n") | ForEach-Object {if($_.StartsWith('          ')){$_.Substring(10)}else{$_}}) -join "`n"
[IO.File]::WriteAllText((Join-Path $testEvidence 'exact-step.ps1'),$stepText,[Text.UTF8Encoding]::new($false))
$testStep=[scriptblock]::Create($stepText)
function Check($condition,[string]$label){if(-not $condition){throw "Synthetic assertion: $label"}}
function Put([string]$name,[string]$text){$dest=Join-Path $env:AR0_ROOT $name;[IO.Directory]::CreateDirectory((Split-Path $dest -Parent)) | Out-Null;[IO.File]::WriteAllText($dest,$text,[Text.UTF8Encoding]::new($false))}
$env:GITHUB_SHA='1111111111111111111111111111111111111111';$env:GITHUB_RUN_ID='123';$env:GITHUB_RUN_ATTEMPT='1'
$cases=@('failed','truncated','invalid-ledger','expected-guards','missing-receipt','oversize-log')
foreach($caseName in $cases){
 $env:AR0_ROOT=Join-Path $testEvidence $caseName;New-Item -ItemType Directory -Path (Join-Path $env:AR0_ROOT 'public') -Force | Out-Null
 Put 'execution.json' (@{schema='nova.ar0.ci-execution/1';sha=$env:GITHUB_SHA;run_id='123';attempt='1';state='COMPLETE';exit_code=1}|ConvertTo-Json)
 $testCommands=@(@{name='default-required-features';state='COMPLETE';exit_code=101},@{name='default-tests';state='COMPLETE';exit_code=101})
 if($caseName -eq 'missing-receipt'){$testCommands=@(@{name='default-tests';state='PARTIAL';exit_code=$null})}
 if($caseName -eq 'invalid-ledger'){$testCommands=@(@{name='../escape';state='COMPLETE';exit_code=101})}
 if($caseName -eq 'expected-guards'){$testCommands=@(@{name='default-required-features';state='COMPLETE';exit_code=101})}
 Put 'ev/sh1-regression/result.json' (@{schema='nova.scheduler.local-validation/1';commands=$testCommands;failure_detail=@{schema='nova.sh1.failure-detail/1';stage='RELEASE_GUARD';classification='RELEASE_GUARD_ASSERTION';line=91;facts=@{exit_is_101=$true;error_count_is_one=$false};logs=@();compiler_errors=@{total=20;errors=@(@{target_is_nova_lib=$true;message_is_guard=$false;rust_code='E0425';source_category='SELF_CODEX_RS';line=123})*20}}}|ConvertTo-Json -Depth 8)
 Put 'ev/result.json' (@{schema='nova.relay.local-validation/1';commands=@(@{name='sh1-wrapper';state='COMPLETE';exit_code=1})}|ConvertTo-Json -Depth 8)
 $secret='SYNTHETIC_SECRET_MUST_NOT_EXPORT_8fd372'
 $failureText="test scheduler::synthetic::fails ... FAILED`n---- scheduler::synthetic::fails stdout ----`nthread 'scheduler::synthetic::fails' panicked at C:\Users\$secret\scheduler\example.rs:42:7:`nassertion ``left == right`` failed: $secret`n  left: `"$secret`"`n right: `"safe`"`nAuthorization: Bearer $secret`nOPENAI_API_KEY=$secret`nraw arbitrary error $secret`ntest result: FAILED. 907 passed; 1 failed; 19 ignored; 0 measured; 0 filtered out; finished in 957.25s`n"
 if($caseName -eq 'truncated'){for($i=0;$i -lt 12;$i++){$failureText+="test scheduler::synthetic::fails$i ... FAILED`n"};$failureText+=('x'*2048)+$secret+"`n"}
 if($caseName -eq 'oversize-log'){$failureText='x'*(16777216+1)}
 Put 'ev/sh1-regression/default-tests.stdout.log' $failureText
 Put 'ev/sh1-regression/default-tests.stderr.log' "error[E0308]: $secret`npassword=$secret`n"
 Put 'ev/sh1-wrapper.stdout.log' '';Put 'ev/sh1-wrapper.stderr.log' "arbitrary wrapper error $secret`n"
 try {. $testStep;throw 'Expected verification rejection'}catch{Check ($_.Exception.Message -eq 'AR0_EXECUTION_REJECTED') 'verdict unchanged'}
 $testSummary=Get-Content -Raw (Join-Path $env:AR0_ROOT 'public/summary.json') | ConvertFrom-Json -AsHashtable
 $testLogsRaw=Get-Content -Raw (Join-Path $env:AR0_ROOT 'public/logs.json');$testLogs=$testLogsRaw | ConvertFrom-Json -AsHashtable
 Check ($testSummary.verdict -eq 'FAIL') 'failure retained';Check (-not $testLogsRaw.Contains($secret)) 'secret excluded'
 Check ($testLogs.failure_detail.wrapper.exit_code -eq 1) 'wrapper actual exit'
 if($caseName -cin @('failed','truncated')){
  $testFirst=$testLogs.failure_detail.records[0];Check ($testFirst.command -eq 'default-tests' -and $testFirst.exit_code -eq 101 -and $testFirst.expected_exit_code -eq 0) 'inner actual exit and expected guard excluded'
  Check ($testFirst.stdout.failed_tests -contains 'scheduler::synthetic::fails') 'failed name'
  Check ($testFirst.stdout.context -contains 'PANIC_AT example.rs:42:7') 'source location path stripped'
  Check ($testFirst.stdout.context -contains 'ASSERTION_FAILED; expression/message withheld') 'assertion context'
  Check ($testFirst.stderr.context -contains 'RUST_ERROR_E0308; message withheld') 'stderr context'
  Check ($testFirst.stdout.sha256 -eq (Get-FileHash (Join-Path $env:AR0_ROOT 'ev/sh1-regression/default-tests.stdout.log')).Hash.ToLowerInvariant()) 'original hash'
  if($caseName -eq 'truncated'){Check ($testFirst.stdout.truncated -and $testFirst.stdout.failed_tests.Count -eq 4) 'truncation retained hash'}
 }
 if($caseName -eq 'invalid-ledger'){Check ($testLogs.failure_detail.ledgers[0].state -eq 'MISSING_INVALID_OR_OVERSIZE') 'unknown command rejected';Check (-not $testLogsRaw.Contains('../escape')) 'unknown name withheld'}
 if($caseName -eq 'missing-receipt'){Check ($testLogs.failure_detail.records[0].state -eq 'PARTIAL' -and $null -eq $testLogs.failure_detail.records[0].exit_code) 'unknown exit never guessed'}
 if($caseName -eq 'oversize-log'){Check ($testLogs.failure_detail.records[0].stdout.state -eq 'MISSING_INVALID_OR_OVERSIZE_NOT_EXPORTED' -and $testLogs.failure_detail.records[0].stdout.sha256 -eq (Get-FileHash (Join-Path $env:AR0_ROOT 'ev/sh1-regression/default-tests.stdout.log')).Hash.ToLowerInvariant()) 'oversize context refused original streaming hash'}
 if($caseName -eq 'expected-guards'){Check ($testLogs.failure_detail.records.Count -eq 1 -and $testLogs.failure_detail.records[0].command -eq 'sh1-wrapper') 'expected nonzero not failure'}
 Check ($testLogs.sh1_failure.classification -eq 'RELEASE_GUARD_ASSERTION' -and $testLogs.sh1_failure.facts.error_count_is_one -ceq $false) 'nested SH1 bounded diagnostic preserved'
 Check ($testLogs.sh1_failure.compiler_errors.total -eq 20 -and $testLogs.sh1_failure.compiler_errors.errors.Count -eq 16 -and $testLogs.sh1_failure.compiler_errors.truncated) 'nested compiler identities bounded and retained'
 $testManifest=Get-Content -Raw (Join-Path $env:AR0_ROOT 'public/files.json') | ConvertFrom-Json -AsHashtable
 Check (@(Get-ChildItem (Join-Path $env:AR0_ROOT 'public') -File).Count -eq 4) 'four files unchanged'
 foreach($entry in $testManifest.files){$file=Join-Path $env:AR0_ROOT "public/$($entry.name)";Check ((Get-Item $file).Length -eq $entry.bytes -and (Get-FileHash $file).Hash.ToLowerInvariant() -eq $entry.sha256) 'manifest';Check ((Get-Content -Raw $file | ConvertFrom-Json).identity.sha -eq $env:GITHUB_SHA) 'identity'}
 Write-Output "PASS synthetic:$caseName"
}
Write-Output 'AR0_FAILURE_EXPORT_SYNTHETIC_PASS'
