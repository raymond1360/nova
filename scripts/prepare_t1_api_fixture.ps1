#requires -Version 7.0
param(
 [Parameter(Mandatory=$true,ParameterSetName='Fixture')][string]$RunDirectory,
 [Parameter(Mandatory=$true,ParameterSetName='Dependencies')]
 [Parameter(Mandatory=$true,ParameterSetName='Evidence')][string]$VerifierDeps,
 [Parameter(Mandatory=$true,ParameterSetName='Evidence')][switch]$ExportDiagnostics,
 [Parameter(Mandatory=$true,ParameterSetName='Evidence')][string]$DiagnosticDirectory,
 [Parameter(Mandatory=$true,ParameterSetName='Dependencies')][string]$NodePath,
 [Parameter(ParameterSetName='Dependencies')][string]$GitPath='C:\Program Files\Git\cmd\git.exe',
 [Parameter(ParameterSetName='Dependencies')][switch]$PreparationSelfTest
)
$ErrorActionPreference='Stop'
$root=Split-Path -Parent $PSScriptRoot
# Evidence export never runs a child, prepares dependencies, or copies raw logs.
# Only these direct children can be read; all other files/directories are excluded.
if($PSCmdlet.ParameterSetName -eq 'Evidence') {
 $names=@('preparation-identity.json','historical-inputs.json','node-version.stdout.txt','node-version.stderr.txt','node-version.exit.json','git-version.stdout.txt','git-version.stderr.txt','git-version.exit.json','dependencies.stdout.txt','dependencies.stderr.txt','dependencies.exit.json','preparation-failure.json','preparation-complete.json','npm-ci-result.json','npm-ci.stdout.bin','npm-ci.stderr.bin','source-rules-selftest.json')
 $maxFile=65536;$maxTotal=1048576;$total=0;$outputTotal=0;$rows=@()
 $source=[IO.Path]::GetFullPath($VerifierDeps);$dest=[IO.Path]::GetFullPath($DiagnosticDirectory)
 if($dest -eq $source -or $dest.StartsWith($source.TrimEnd('\')+'\',[StringComparison]::OrdinalIgnoreCase)){throw 'Diagnostics must be separate from preparation'}
 function Test-PlainAncestors([string]$Path) {
  $cursor=$Path
  while($cursor){
   if(Test-Path -LiteralPath $cursor){if((Get-Item -LiteralPath $cursor -Force).Attributes -band [IO.FileAttributes]::ReparsePoint){throw 'Diagnostic reparse ancestor rejected'}}
   $parent=Split-Path -Parent $cursor;if($parent -eq $cursor){break};$cursor=$parent
  }
 }
 Test-PlainAncestors $source;Test-PlainAncestors $dest
 if(Test-Path -LiteralPath $dest){throw 'Fresh diagnostic directory required'}
 New-Item -ItemType Directory -Path $dest | Out-Null
 foreach($name in $names){
  $path=Join-Path $source $name;$row=[ordered]@{name=$name;state='MISSING';source_bytes=$null;source_sha256=$null;output_bytes=$null;output_sha256=$null;redacted=$false};$rows+=,$row
  if(-not(Test-Path -LiteralPath $path)){continue}
  try {
   $item=Get-Item -LiteralPath $path -Force
   if($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)){$row.state='REJECTED_TYPE';continue}
   $stream=[IO.File]::Open($path,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::Read)
   try {
    $row.source_bytes=$stream.Length
    if($stream.Length -gt $maxFile -or $total+$stream.Length -gt $maxTotal){$row.state='REJECTED_SIZE';continue}
    $bytes=[byte[]]::new([int]$stream.Length);$offset=0
    while($offset -lt $bytes.Length){$count=$stream.Read($bytes,$offset,$bytes.Length-$offset);if($count -eq 0){throw 'Diagnostic short read'};$offset+=$count}
   } finally {$stream.Dispose()}
   $total+=$bytes.Length;$row.source_sha256=[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($bytes)).ToLowerInvariant()
   $text=[Text.UTF8Encoding]::new($false,$true).GetString($bytes).TrimStart([char]0xfeff)
   if($name.EndsWith('.json')){
    $obj=ConvertFrom-Json -InputObject $text -AsHashtable -Depth 32
    if($obj -isnot [Collections.IDictionary]){throw 'Diagnostic JSON object required'}
    # Reconstruct typed, fixed fields. Never serialize arbitrary error strings,
    # paths, environment, records, URLs, headers, command argv, or extra keys.
    $safe=[ordered]@{sanitized=$true;source_name=$name}
    foreach($key in @('elapsed_ms','packages','plans','negative','scoped')){
     if($obj.Contains($key) -and $obj[$key] -is [long] -and $obj[$key] -ge 0 -and $obj[$key] -le 2147483647){$safe[$key]=$obj[$key]}
    }
    if($obj.Contains('exit') -and ($null -eq $obj.exit -or ($obj.exit -is [long] -and $obj.exit -ge -2147483648 -and $obj.exit -le 2147483647))){$safe.exit=$obj.exit}
    if($obj.Contains('utc')){try{if($obj.utc -is [datetime]){$safe.utc=$obj.utc.ToUniversalTime().ToString('o')}elseif($obj.utc -is [string]){$safe.utc=[DateTimeOffset]::ParseExact($obj.utc,'o',[Globalization.CultureInfo]::InvariantCulture).ToString('o')}}catch{}}
    foreach($key in @('timed_out','inherited_environment')){if($obj.Contains($key) -and $obj[$key] -is [bool]){$safe[$key]=$obj[$key]}}
    foreach($key in @('script_sha256','node_sha256','git_sha256','lock_before','lock_after')){
     if($obj.Contains($key) -and [string]$obj[$key] -cmatch '^[0-9a-f]{64}$'){$safe[$key]=[string]$obj[$key]}
    }
    if($name -eq 'historical-inputs.json'){
     if($obj.commit -cne '7788e9bfd6d7fc6baccce503b0ad5d873790d026'){throw 'Unexpected history evidence'}
     $safe.commit=$obj.commit;$safe.verified=($obj.verified -is [bool] -and $obj.verified -eq $true)
     $safe.inputs=[ordered]@{}
     foreach($f in @('package.json','package-lock.json')){
      $input=$obj.inputs[$f]
      $expectedSize=if($f -eq 'package.json'){628}else{104076}
      $expectedHash=if($f -eq 'package.json'){'f1256aec72b0eae47b8cc3bef4937a1284bb9cf69d94cd1eaaa4df77b460f991'}else{'097850e97d14a6280dbc2ca91b411ecc4aa6bebf327a5a97d8d647a07fe5b68a'}
      $expectedBlob=if($f -eq 'package.json'){'6d682980eb0910c5e17afd3fb29ea78e81636bc8'}else{'3632f1d248f294f73dae78a56c02c6c9355b7f52'}
      if($input.size -ne $expectedSize -or $input.sha256 -cne $expectedHash -or $input.blob -cne $expectedBlob){throw 'Unexpected history evidence identity'}
      $safe.inputs[$f]=@{size=$expectedSize;sha256=$expectedHash;blob=$expectedBlob}
     }
    }
    if($name -in @('preparation-failure.json','npm-ci-result.json')){
     if($obj.error_code -is [string] -and $obj.error_code.Length -le 64 -and $obj.error_code -cmatch '\A(E[A-Z0-9_]{2,40}|ERR_[A-Z0-9_]{2,60}|[A-Z][A-Z0-9_]{3,60})\z'){$safe.error_code=$obj.error_code}
    }
    if($name -eq 'npm-ci-result.json'){
     if($obj.signal -is [string] -and $obj.signal.Length -le 32 -and $obj.signal -cmatch '\ASIG[A-Z]+\z'){$safe.signal=$obj.signal}
    }
    if($name -eq 'preparation-failure.json'){
     $stages=@('HISTORY_COMMIT','HISTORY_BLOB','LOCK','PLAN','DOWNLOAD_HOP0','DOWNLOAD_HOP1','INTEGRITY','CACHE_WRITE','NPM_CI')
     if($obj.stage -is [string] -and $stages -ccontains $obj.stage){$safe.stage=$obj.stage}
     if($obj.record_index -is [long] -and $obj.record_index -ge 0 -and $obj.record_index -lt 198){$safe.record_index=$obj.record_index}
     if($obj.http_status -is [long] -and $obj.http_status -ge 100 -and $obj.http_status -le 599){$safe.http_status=$obj.http_status}
     if($obj.git_status -is [long] -and $obj.git_status -ge -2147483648 -and $obj.git_status -le 2147483647){$safe.git_status=$obj.git_status}
     if($obj.git_class -is [string] -and @('GIT_DUBIOUS_OWNERSHIP','GIT_NOT_REPOSITORY','GIT_OBJECT_MISSING','GIT_OTHER') -ccontains $obj.git_class){$safe.git_class=$obj.git_class}

     $safe.error='[REDACTED: arbitrary child error]'
     if($obj.error -is [string] -and $obj.error -cmatch '^Error: historical input (commit missing or invalid|blob identity mismatch: package(-lock)?\.json)$'){$safe.error=$obj.error}
     $knownErrors=@('fixed Node version/arch','lock blob mismatch','full package count','single sha512 required','lock key invalid','package identity invalid','unapproved exact source','source/package mapping','redirect status/hop/HTTPS rejected','fixed CDN mapping rejected','noncanonical CDN URL','second response must be 200; no further redirect','locked sha512 mismatch','offline npm ci failed or changed lock','overall preparation deadline','absolute request deadline','package size bound')
     if($obj.error -is [string] -and $obj.error.StartsWith('Error: ') -and $knownErrors -ccontains $obj.error.Substring(7)){$safe.error=$obj.error}
     $safe.record_count=if($obj.records -is [array]){$obj.records.Count}else{$null}
    }
    $output=ConvertTo-Json -InputObject $safe -Depth 5;$row.redacted=$true
   } else {
    $lines=[Collections.Generic.List[string]]::new()
    if($text.Length -gt 0){foreach($line in ($text -split '\r?\n')){
     if($line -ceq 'v24.14.0' -or $line -cmatch '^git version 2\.[0-9]{1,3}\.[0-9]{1,3}(\.windows\.[0-9]{1,3})?$'){$lines.Add($line)}
     elseif($name -in @('npm-ci.stdout.bin','npm-ci.stderr.bin') -and $line -cmatch '\Anpm (error|ERR!) code [A-Z0-9_]{1,64}\z'){$lines.Add($line)}
     elseif($name -in @('npm-ci.stdout.bin','npm-ci.stderr.bin') -and $line -cmatch '\Anpm (error|ERR!) errno -?[0-9]{1,10}\z'){
      $errno=[long](($line -split ' ')[-1]);if($errno -ge -2147483648 -and $errno -le 2147483647){$lines.Add($line)}else{$lines.Add('[REDACTED: unapproved log line]');$row.redacted=$true}
     }
     elseif($line.Length -gt 0){$lines.Add('[REDACTED: unapproved log line]');$row.redacted=$true}
    }}
    $output=$lines -join "`n"
   }
   $encoded=[Text.UTF8Encoding]::new($false).GetBytes($output)
   if($encoded.Length -gt $maxFile -or $outputTotal+$encoded.Length -gt $maxTotal){$row.state='REJECTED_OUTPUT_SIZE';continue}
   [IO.File]::WriteAllBytes((Join-Path $dest $name),$encoded)
   $outputTotal+=$encoded.Length;$row.output_bytes=$encoded.Length;$row.output_sha256=[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($encoded)).ToLowerInvariant()
   $row.state=if($bytes.Length -eq 0){'EMPTY'}else{'SANITIZED'}
  } catch {$row.state='REJECTED_READ_OR_PARSE';$row.redacted=$true}
 }
 $manifest=[ordered]@{purpose='CI diagnostics only; not raw preparation evidence';policy='fixed direct-child allowlist; unknown text/URLs removed; no environment or paths';per_file_limit=$maxFile;total_input_limit=$maxTotal;total_read=$total;total_output_limit=$maxTotal;total_output=$outputTotal;files=$rows}
 [IO.File]::WriteAllText((Join-Path $dest 'diagnostic-manifest.json'),($manifest|ConvertTo-Json -Depth 6),[Text.UTF8Encoding]::new($false))
 Write-Output 'Exported fixed sanitized CI diagnostics; see manifest for missing/rejected/empty files.'
 return
}
if($PSCmdlet.ParameterSetName -eq 'Dependencies') {
 if(-not [IO.Path]::IsPathRooted($NodePath) -or (Get-FileHash -LiteralPath $NodePath -Algorithm SHA256).Hash.ToLowerInvariant() -cne '63c259c81e5d472b5f11c8d506070130cb04a1ecf84b80377a34ed6ec9048088'){throw 'Pinned Windows x64 Node required'}
 if(-not [IO.Path]::IsPathRooted($GitPath) -or -not(Test-Path -LiteralPath $GitPath -PathType Leaf)){throw 'Verified absolute Git required'}
 $NodePath=(Resolve-Path -LiteralPath $NodePath).Path;$GitPath=(Resolve-Path -LiteralPath $GitPath).Path
 if(Test-Path -LiteralPath $VerifierDeps){throw 'Fresh isolated verifier directory required'}
 $VerifierDeps=[IO.Path]::GetFullPath($VerifierDeps)
 New-Item -ItemType Directory -Path $VerifierDeps | Out-Null
 $owned=Join-Path $VerifierDeps 'owned-environment';New-Item -ItemType Directory -Path $owned | Out-Null
 foreach($name in @('temp','home','appdata','localappdata')){New-Item -ItemType Directory -Path (Join-Path $owned $name) | Out-Null}
 foreach($name in @('npm-user.npmrc','npm-global.npmrc','git-empty.config')){[IO.File]::WriteAllBytes((Join-Path $owned $name),[byte[]]@())}
 $system=[Environment]::GetFolderPath([Environment+SpecialFolder]::Windows)
 $clean=@{
  SystemRoot=$system;WINDIR=$system;COMSPEC=(Join-Path $system 'System32/cmd.exe');PATH=((Split-Path $NodePath),(Split-Path $GitPath),(Join-Path $system 'System32') -join ';');
  TEMP=(Join-Path $owned 'temp');TMP=(Join-Path $owned 'temp');HOME=(Join-Path $owned 'home');USERPROFILE=(Join-Path $owned 'home');APPDATA=(Join-Path $owned 'appdata');LOCALAPPDATA=(Join-Path $owned 'localappdata');
  NPM_CONFIG_USERCONFIG=(Join-Path $owned 'npm-user.npmrc');NPM_CONFIG_GLOBALCONFIG=(Join-Path $owned 'npm-global.npmrc');NPM_CONFIG_CACHE=(Join-Path $VerifierDeps 'npm-cache');NPM_CONFIG_IGNORE_SCRIPTS='true';NPM_CONFIG_AUDIT='false';NPM_CONFIG_FUND='false';NPM_CONFIG_OFFLINE='true';NPM_CONFIG_REGISTRY='https://registry.npmmirror.com';
  GIT_CONFIG_NOSYSTEM='1';GIT_CONFIG_GLOBAL=(Join-Path $owned 'git-empty.config');GIT_CONFIG_SYSTEM=(Join-Path $owned 'git-empty.config');GIT_CONFIG_COUNT='0';GIT_TERMINAL_PROMPT='0'
 }
 function Invoke-Clean([string]$Exe,[string[]]$Vector,[string]$Name,[int]$Timeout) {
  $info=[Diagnostics.ProcessStartInfo]::new();$info.FileName=$Exe;$info.WorkingDirectory=$root;$info.UseShellExecute=$false;$info.CreateNoWindow=$true;$info.RedirectStandardOutput=$true;$info.RedirectStandardError=$true
  $info.Environment.Clear();foreach($key in $clean.Keys){$info.Environment[$key]=$clean[$key]};foreach($arg in $Vector){$info.ArgumentList.Add($arg)}
  $p=[Diagnostics.Process]::new();$p.StartInfo=$info;if(-not $p.Start()){throw 'Preparation start failed'}
  $out=$p.StandardOutput.ReadToEndAsync();$err=$p.StandardError.ReadToEndAsync();$timeoutHit=-not $p.WaitForExit($Timeout)
  if($timeoutHit){$p.Kill($true);$p.WaitForExit()}
  [IO.File]::WriteAllText((Join-Path $VerifierDeps ($Name+'.stdout.txt')),$out.GetAwaiter().GetResult(),[Text.UTF8Encoding]::new($false))
  [IO.File]::WriteAllText((Join-Path $VerifierDeps ($Name+'.stderr.txt')),$err.GetAwaiter().GetResult(),[Text.UTF8Encoding]::new($false))
  $exit=$p.ExitCode;$p.Dispose()
  @{exit=$exit;timed_out=$timeoutHit;environment_keys=@($clean.Keys|Sort-Object)} | ConvertTo-Json | Set-Content (Join-Path $VerifierDeps ($Name+'.exit.json')) -Encoding utf8
  if($timeoutHit -or $exit -ne 0){throw "Preparation $Name failed: exit=$exit timeout=$timeoutHit; no retry"}
 }
 Invoke-Clean $NodePath @('--version') 'node-version' 10000
 if(([IO.File]::ReadAllText((Join-Path $VerifierDeps 'node-version.stdout.txt'))).Trim() -cne 'v24.14.0'){throw 'Node version mismatch'}
 Invoke-Clean $GitPath @('--version') 'git-version' 10000
 if(([IO.File]::ReadAllText((Join-Path $VerifierDeps 'git-version.stdout.txt'))) -notmatch '^git version 2\.') {throw 'Git version mismatch'}
 @{utc=[DateTimeOffset]::UtcNow.ToString('o');script_sha256=(Get-FileHash -LiteralPath $PSCommandPath).Hash.ToLowerInvariant();node_path=$NodePath;node_sha256=(Get-FileHash -LiteralPath $NodePath).Hash.ToLowerInvariant();git_path=$GitPath;git_sha256=(Get-FileHash -LiteralPath $GitPath).Hash.ToLowerInvariant();environment_keys=@($clean.Keys|Sort-Object);inherited_environment=$false;tls='default verification; no proxy or extra CA'} | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $VerifierDeps 'preparation-identity.json') -Encoding utf8
 $prepare=@'
'use strict';
const fs=require('node:fs'),path=require('node:path'),https=require('node:https'),crypto=require('node:crypto'),cp=require('node:child_process'),assert=require('node:assert/strict');
const {createRequire}=require('node:module');const [dir,git,mode]=process.argv.slice(1),d=path.resolve(dir);
const identity=JSON.parse(fs.readFileSync(path.join(d,'preparation-identity.json'),'utf8').replace(/^\uFEFF/,''));
const started=Date.now(),overall=20*60*1000,perRequest=30000,MAX=50*1024*1024,records=[];
const forbidden=/^(NODE_OPTIONS|NODE_PATH|NODE_TLS_REJECT_UNAUTHORIZED|NODE_EXTRA_CA_CERTS|NODE_USE_ENV_PROXY|SSL_CERT_FILE|SSL_CERT_DIR|HTTPS?_PROXY|ALL_PROXY|NO_PROXY|CURL_CA_BUNDLE|REQUESTS_CA_BUNDLE)$/i;
for(const k of Object.keys(process.env))if(forbidden.test(k))throw Error('inherited injection environment key');
// Diagnostic-only state: sequential preparation has at most one active package.
const STAGES=['HISTORY_COMMIT','HISTORY_BLOB','LOCK','PLAN','DOWNLOAD_HOP0','DOWNLOAD_HOP1','INTEGRITY','CACHE_WRITE','NPM_CI'];
let stage,recordIndex,httpStatus;
function enterStage(next,index,status){
 if(!STAGES.includes(next))throw Error('invalid internal stage');
 stage=next;recordIndex=Number.isInteger(index)&&index>=0&&index<198?index:undefined;
 httpStatus=Number.isInteger(status)&&status>=100&&status<=599?status:undefined;
}
function errorCode(value){
 if(typeof value!=='string'||value.length>64)return undefined;
 const m=/^(E[A-Z0-9_]{2,40}|ERR_[A-Z0-9_]{2,60}|[A-Z][A-Z0-9_]{3,60})$/.exec(value);
 return m&&m[0]===value?value:undefined;
}
function signalCode(value){return typeof value==='string'&&value.length<=32&&/^SIG[A-Z]+$/.exec(value)?.[0]===value?value:undefined;}
function gitClass(stderr){
 const text=Buffer.isBuffer(stderr)?stderr.toString('utf8'):typeof stderr==='string'?stderr:'';
 if(text.includes('dubious ownership'))return 'GIT_DUBIOUS_OWNERSHIP';
 if(text.includes('not a git repository'))return 'GIT_NOT_REPOSITORY';
 if(['Needed a single revision','unknown revision','bad object','could not get object info'].some(x=>text.includes(x)))return 'GIT_OBJECT_MISSING';
 return 'GIT_OTHER';
}
function gitFailure(original,message){
 const wrapped=Error(message);wrapped.git_class=gitClass(original?.stderr);
 if(Number.isInteger(original?.status)&&original.status>=-2147483648&&original.status<=2147483647)wrapped.git_status=original.status;
 const code=errorCode(original?.code);if(code!==undefined)wrapped.code=code;
 return wrapped;
}
function failureFields(e){
 const result={};if(STAGES.includes(stage))result.stage=stage;
 if(recordIndex!==undefined)result.record_index=recordIndex;
 if(httpStatus!==undefined)result.http_status=httpStatus;
 const code=errorCode(e?.code);if(code!==undefined)result.error_code=code;
 if(['GIT_DUBIOUS_OWNERSHIP','GIT_NOT_REPOSITORY','GIT_OBJECT_MISSING','GIT_OTHER'].includes(e?.git_class))result.git_class=e.git_class;
 if(Number.isInteger(e?.git_status)&&e.git_status>=-2147483648&&e.git_status<=2147483647)result.git_status=e.git_status;
 return result;
}
function writeNew(name,value){fs.writeFileSync(path.join(d,name),JSON.stringify(value,null,2),{flag:'wx'});}
function remaining(){const ms=overall-(Date.now()-started);if(ms<=0)throw Error('overall preparation deadline');return ms;}
function progress(){fs.writeFileSync(path.join(d,'download-progress.json'),JSON.stringify({records},null,2));}
function plan(key,p){
 if(!/^sha512-[A-Za-z0-9+/]{86}==$/.test(p.integrity||''))throw Error('single sha512 required');
 const at=key.lastIndexOf('node_modules/');if(at<0)throw Error('lock key invalid');const name=p.name||key.slice(at+13);
 if(!/^(@[a-z0-9_.-]+\/)?[a-z0-9_.-]+$/i.test(name)||!/^[A-Za-z0-9.+_-]+$/.test(p.version))throw Error('package identity invalid');
 const u=new URL(p.resolved);if(u.origin!=='https://registry.npmmirror.com'||u.username||u.password||u.search||u.hash)throw Error('unapproved exact source');
 const file=u.pathname.split('/').at(-1);if(file!==name.split('/').at(-1)+'-'+p.version+'.tgz'||decodeURIComponent(u.pathname)!=='/'+name+'/-/'+file)throw Error('source/package mapping');
 // Scoped name has two independently encoded segments; its slash is retained.
 return {lock_key:key,name,version:p.version,origin:p.resolved,integrity:p.integrity,expected_path:'/packages/'+name.split('/').map(encodeURIComponent).join('/')+'/'+p.version+'/'+file};
}
function redirect(p,location,status,hop){
 if(hop!==0||![301,302,307,308].includes(status)||typeof location!=='string'||!location.startsWith('https://'))throw Error('redirect status/hop/HTTPS rejected');
 const u=new URL(location);if(u.protocol!=='https:'||u.hostname!=='cdn.npmmirror.com'||u.port!==''||u.username||u.password||u.search||u.hash||u.pathname!==p.expected_path)throw Error('fixed CDN mapping rejected');
 if(location!=='https://cdn.npmmirror.com'+p.expected_path&&location!=='https://cdn.npmmirror.com:443'+p.expected_path)throw Error('noncanonical CDN URL');return location;
}
function get(url,row,hop,index){enterStage(hop===0?'DOWNLOAD_HOP0':'DOWNLOAD_HOP1',index);return new Promise((ok,no)=>{
 const agent=new https.Agent({keepAlive:false});let done=false,size=0,chunks=[];
 const request=https.get(url,{agent,rejectUnauthorized:true,headers:{Accept:'application/octet-stream'}},response=>{
  httpStatus=Number.isInteger(response.statusCode)&&response.statusCode>=100&&response.statusCode<=599?response.statusCode:undefined;
  const event={url,hop,status:response.statusCode,location:response.headers.location||null,bytes:0};row.responses.push(event);
  if(response.statusCode!==200){response.resume();finish(null,{status:response.statusCode,location:response.headers.location});return;}
  response.on('data',b=>{size+=b.length;event.bytes=size;if(size>MAX){request.destroy(Error('package size bound'));return;}chunks.push(b);});
  response.on('end',()=>finish(null,{status:200,bytes:Buffer.concat(chunks)}));response.on('error',e=>finish(e));
 });
 const timer=setTimeout(()=>request.destroy(Error('absolute request deadline')),Math.min(perRequest,remaining()));
 function finish(e,v){if(done)return;done=true;clearTimeout(timer);agent.destroy();if(e)no(e);else ok(v);}
 request.on('error',e=>finish(e));
});}
(async()=>{try{
 if(process.version!=='v24.14.0'||process.arch!=='x64')throw Error('fixed Node version/arch');
 const base='7788e9bfd6d7fc6baccce503b0ad5d873790d026',gitOptions={env:process.env,timeout:10000,maxBuffer:1024*1024};
 enterStage('HISTORY_COMMIT');
 try{if(cp.execFileSync(git,['rev-parse','--verify',base+'^{commit}'],gitOptions).toString().trim()!==base||cp.execFileSync(git,['cat-file','-t',base],gitOptions).toString().trim()!=='commit')throw Error('wrong commit');}catch(e){throw gitFailure(e,'historical input commit missing or invalid');}
 const expected={'package.json':{size:628,sha256:'f1256aec72b0eae47b8cc3bef4937a1284bb9cf69d94cd1eaaa4df77b460f991',blob:'6d682980eb0910c5e17afd3fb29ea78e81636bc8'},'package-lock.json':{size:104076,sha256:'097850e97d14a6280dbc2ca91b411ecc4aa6bebf327a5a97d8d647a07fe5b68a',blob:'3632f1d248f294f73dae78a56c02c6c9355b7f52'}};
 for(const [f,e] of Object.entries(expected)){
  enterStage('HISTORY_BLOB');
  let bytes,blob;try{bytes=cp.execFileSync(git,['--no-pager','show',base+':apps/desktop/'+f],gitOptions);blob=cp.execFileSync(git,['rev-parse',base+':apps/desktop/'+f],gitOptions).toString().trim();}catch(e){throw gitFailure(e,'historical input blob identity mismatch: '+f);}
  if(blob!==e.blob||bytes.length!==e.size||crypto.createHash('sha256').update(bytes).digest('hex')!==e.sha256)throw Error('historical input blob identity mismatch: '+f);
  fs.writeFileSync(path.join(d,f),bytes,{flag:'wx'});
 }
 writeNew('historical-inputs.json',{commit:base,verified:true,inputs:expected});
 enterStage('LOCK');
 const lockBytes=fs.readFileSync(path.join(d,'package-lock.json'));if(lockBytes.length!==104076||crypto.createHash('sha256').update(lockBytes).digest('hex')!=='097850e97d14a6280dbc2ca91b411ecc4aa6bebf327a5a97d8d647a07fe5b68a')throw Error('lock blob mismatch');
 enterStage('PLAN');
 const plans=Object.entries(JSON.parse(lockBytes).packages).filter(([,p])=>p.resolved).map(([k,p])=>plan(k,p));if(plans.length!==198)throw Error('full package count');writeNew('source-plan.json',plans);
 if(mode==='selftest'){
  for(const p of plans)assert.equal(redirect(p,'https://cdn.npmmirror.com'+p.expected_path,302,0),'https://cdn.npmmirror.com'+p.expected_path);
  const p=plans[0];let negative=0;for(const [u,s,h]of [['//cdn.npmmirror.com'+p.expected_path,302,0],['http://cdn.npmmirror.com'+p.expected_path,302,0],['https://cdn.npmmirror.com.evil'+p.expected_path,302,0],['https://user:pw@cdn.npmmirror.com'+p.expected_path,302,0],['https://cdn.npmmirror.com:444'+p.expected_path,302,0],['https://cdn.npmmirror.com'+p.expected_path+'?x=1',302,0],['https://cdn.npmmirror.com'+p.expected_path+'#x',302,0],['https://cdn.npmmirror.com'+p.expected_path,303,0],['https://cdn.npmmirror.com'+p.expected_path,302,1],['https://cdn.npmmirror.com/packages/other/1/x.tgz',302,0]]){assert.throws(()=>redirect(p,u,s,h));negative++;}
  writeNew('source-rules-selftest.json',{source:'SYNTHETIC no network',plans:plans.length,negative,scoped:plans.filter(p=>p.name.startsWith('@')).length});return;
 }
 enterStage('CACHE_WRITE');
 const npmRoot=path.join(path.dirname(process.execPath),'node_modules/npm'),requireNpm=createRequire(path.join(npmRoot,'package.json')),cacache=requireNpm('cacache');
 for(const [index,p] of plans.entries()){enterStage('DOWNLOAD_HOP0',index);remaining();const row={...p,utc:new Date().toISOString(),responses:[],integrity_verified:false,cache_written:false};records.push(row);try{
  let result=await get(p.origin,row,0,index);if(result.status!==200){result=await get(redirect(p,result.location,result.status,0),row,1,index);if(result.status!==200)throw Error('second response must be 200; no further redirect');}
  enterStage('INTEGRITY',index,result.status);
  const bytes=result.bytes;row.bytes=bytes.length;row.sha256=crypto.createHash('sha256').update(bytes).digest('hex');if('sha512-'+crypto.createHash('sha512').update(bytes).digest('base64')!==p.integrity)throw Error('locked sha512 mismatch');row.integrity_verified=true;
  enterStage('CACHE_WRITE',index,result.status);
  await cacache.put(path.join(d,'npm-cache/_cacache'),'make-fetch-happen:request-cache:'+p.origin,bytes,{integrity:p.integrity});row.cache_written=true;progress();
 }catch(e){row.error=String(e);progress();throw e;}}
 enterStage('CACHE_WRITE');
 writeNew('dependency-retrievals.json',{script_sha256:identity.script_sha256,records});
 enterStage('NPM_CI');
 const args=[path.join(npmRoot,'bin/npm-cli.js'),'ci','--ignore-scripts','--no-audit','--no-fund','--offline'],before=crypto.createHash('sha256').update(lockBytes).digest('hex');
 const run=cp.spawnSync(process.execPath,args,{cwd:d,env:process.env,timeout:Math.min(5*60*1000,remaining()),maxBuffer:8*1024*1024});
 fs.writeFileSync(path.join(d,'npm-ci.stdout.bin'),run.stdout||Buffer.alloc(0),{flag:'wx'});fs.writeFileSync(path.join(d,'npm-ci.stderr.bin'),run.stderr||Buffer.alloc(0),{flag:'wx'});
 const after=crypto.createHash('sha256').update(fs.readFileSync(path.join(d,'package-lock.json'))).digest('hex');writeNew('npm-ci-result.json',{argv:args.slice(1),exit:run.status,error_code:errorCode(run.error?.code)??null,signal:signalCode(run.signal)??null,lock_before:before,lock_after:after,elapsed_ms:Date.now()-started});
 if(run.error||run.status!==0||before!==after){const e=Error('offline npm ci failed or changed lock');const code=errorCode(run.error?.code);if(code!==undefined)e.code=code;throw e;}writeNew('preparation-complete.json',{packages:records.length,elapsed_ms:Date.now()-started,script_sha256:identity.script_sha256});
}catch(e){writeNew('preparation-failure.json',{error:String(e),...failureFields(e),elapsed_ms:Date.now()-started,records});console.error(String(e));process.exitCode=1;}})();
'@
 $mode=if($PreparationSelfTest){'selftest'}else{'prepare'}
 Invoke-Clean $NodePath @('-e',$prepare,$VerifierDeps,$GitPath,$mode) 'dependencies' 1230000
 return
}
if(Test-Path -LiteralPath $RunDirectory){throw 'Refusing existing run directory'}
$source=Join-Path $root 'tools/t1-api-eval/fixtures/Logo.tsx'
if((Get-FileHash -LiteralPath $source).Hash.ToLowerInvariant() -cne '0821ae3bbb7e9d2085e9ae9031b4b7744bc191c84a6b7dda6b618b7c1b6ed58a'){throw 'Fixture identity mismatch'}
$project=Join-Path $RunDirectory 'project/src/components/Logo';New-Item -ItemType Directory -Path $project | Out-Null
$target=Join-Path $project 'Logo.tsx';$bytes=[IO.File]::ReadAllBytes($source)
$stream=[IO.File]::Open($target,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::Read)
try{$stream.Write($bytes,0,$bytes.Length);$stream.Flush($true)}finally{$stream.Dispose()}
if((Get-FileHash -LiteralPath $target).Hash.ToLowerInvariant() -cne '0821ae3bbb7e9d2085e9ae9031b4b7744bc191c84a6b7dda6b618b7c1b6ed58a'){throw 'Fixture readback mismatch'}
Write-Output 'Prepared fixed Logo bytes only; no runtime, dependencies or Agent started.'
