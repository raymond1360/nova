# Fixed diagnostic projection only. Never publish exception text, paths, JSON values or environment.
function ConvertTo-Sh1FailureDetail($Value) {
 $out=@{schema='nova.sh1.failure-detail/1';state='MISSING_OR_INVALID';stage='UNKNOWN';classification='UNKNOWN';script='test_scheduler_handoff.ps1';line=$null;column=$null;facts=@{};logs=@()}
 if($Value -isnot [Collections.IDictionary] -or $Value.schema -cne 'nova.sh1.failure-detail/1'){return $out}
 $out.state='PRESENT'
 if($Value.stage -is [string] -and $Value.stage -cin @('BUILD','SCHEDULER_TESTS','DEFAULT_REQUIRED_FEATURES','RELEASE_GUARD','FOUR_COMMAND_FIXTURE','EOL_AND_PATH_FIXTURES','DEFAULT_REGRESSION')){$out.stage=$Value.stage}
 if($Value.classification -is [string] -and $Value.classification -cin @('SCRIPT_EXCEPTION','JSON_PARSE_REJECTED','REQUIRED_FEATURES_ASSERTION','RELEASE_GUARD_ASSERTION','FIXED_COMMAND_FAILED','TOTAL_DEADLINE','LOG_BOUND','FIXTURE_GIT_DEADLINE')){$out.classification=$Value.classification}
 foreach($k in @('line','column')){if(($Value[$k] -is [int] -or $Value[$k] -is [long]) -and $Value[$k] -gt 0 -and $Value[$k] -le 100000){$out[$k]=$Value[$k]}}
 if($Value.facts -is [Collections.IDictionary]){
  foreach($k in @('exit_is_101','required_message_match','could_not_compile_present','json_parse_complete','error_count_is_one','target_is_nova_lib','message_is_guard')){if($Value.facts[$k] -is [bool]){$out.facts[$k]=$Value.facts[$k]}}
  foreach($k in @('json_line','error_count')){if(($Value.facts[$k] -is [int] -or $Value.facts[$k] -is [long]) -and $Value.facts[$k] -ge 0 -and $Value.facts[$k] -le 100000){$out.facts[$k]=$Value.facts[$k]}}
 }
 foreach($row in @($Value.logs) | Select-Object -First 4){
  if($row -is [Collections.IDictionary] -and $row.name -cin @('release-guard.stdout.log','release-guard.stderr.log','default-required-features.stdout.log','default-required-features.stderr.log') -and $row.sha256 -is [string] -and $row.sha256 -cmatch '^[0-9a-f]{64}$' -and ($row.bytes -is [int] -or $row.bytes -is [long]) -and $row.bytes -ge 0){$out.logs+=@{name=$row.name;bytes=$row.bytes;sha256=$row.sha256}}
 }
 return $out
}
function New-Sh1FailureDetail($Record,[string]$Stage,$Facts,[string]$Directory,[string]$ScriptPath) {
 $kind='SCRIPT_EXCEPTION'
 switch -CaseSensitive ($Record.Exception.Message){
  'Not exact required-features rejection'{$kind='REQUIRED_FEATURES_ASSERTION'}
  'Not exact scheduler guard'{$kind='RELEASE_GUARD_ASSERTION'}
  'TOTAL_DEADLINE'{$kind='TOTAL_DEADLINE'}
  'LOG_BOUND'{$kind='LOG_BOUND'}
  'FIXTURE_GIT_DEADLINE'{$kind='FIXTURE_GIT_DEADLINE'}
 }
 if($Stage -ceq 'RELEASE_GUARD' -and $Facts.json_parse_complete -ceq $false){$kind='JSON_PARSE_REJECTED'}
 $v=@{schema='nova.sh1.failure-detail/1';stage=$Stage;classification=$kind;line=$null;column=$null;facts=$Facts;logs=@()}
 if($Record.InvocationInfo.ScriptName -ceq $ScriptPath){$v.line=$Record.InvocationInfo.ScriptLineNumber;$v.column=$Record.InvocationInfo.OffsetInLine}
 foreach($name in @('release-guard.stdout.log','release-guard.stderr.log','default-required-features.stdout.log','default-required-features.stderr.log')){
  $path=Join-Path $Directory $name;$item=Get-Item -LiteralPath $path -ErrorAction SilentlyContinue
  if($item -and -not $item.PSIsContainer -and ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -eq 0){$v.logs+=@{name=$name;bytes=$item.Length;sha256=(Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()}}
 }
 ConvertTo-Sh1FailureDetail $v
}
function Read-Sh1FailureDetail([string]$Path) {
 try {
  $item=Get-Item -LiteralPath $Path -ErrorAction Stop
  if($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or $item.Length -gt 131072){throw 'BOUND'}
  $text=[IO.File]::ReadAllText($item.FullName)
  if([Text.Encoding]::UTF8.GetByteCount($text) -gt 131072){throw 'BOUND'}
  $v=ConvertFrom-Json $text -AsHashtable
  ConvertTo-Sh1FailureDetail $v.failure_detail
 } catch {ConvertTo-Sh1FailureDetail $null}
}
