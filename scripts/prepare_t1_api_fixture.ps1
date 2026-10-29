#requires -Version 7.0
param(
 [Parameter(Mandatory=$true,ParameterSetName='Fixture')][string]$RunDirectory,
 [Parameter(Mandatory=$true,ParameterSetName='Dependencies')][string]$VerifierDeps,
 [Parameter(Mandatory=$true,ParameterSetName='Dependencies')][string]$NodePath,
 [Parameter(ParameterSetName='Dependencies')][string]$GitPath='C:\Program Files\Git\cmd\git.exe',
 [Parameter(ParameterSetName='Dependencies')][switch]$PreparationSelfTest
)
$ErrorActionPreference='Stop'
$root=Split-Path -Parent $PSScriptRoot
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
function get(url,row,hop){return new Promise((ok,no)=>{
 const agent=new https.Agent({keepAlive:false});let done=false,size=0,chunks=[];
 const request=https.get(url,{agent,rejectUnauthorized:true,headers:{Accept:'application/octet-stream'}},response=>{
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
 for(const f of ['package.json','package-lock.json']){const bytes=cp.execFileSync(git,['--no-pager','show','7788e9bfd6d7fc6baccce503b0ad5d873790d026:apps/desktop/'+f],{env:process.env,timeout:10000,maxBuffer:1024*1024});fs.writeFileSync(path.join(d,f),bytes,{flag:'wx'});}
 const lockBytes=fs.readFileSync(path.join(d,'package-lock.json'));if(lockBytes.length!==104076||crypto.createHash('sha256').update(lockBytes).digest('hex')!=='097850e97d14a6280dbc2ca91b411ecc4aa6bebf327a5a97d8d647a07fe5b68a')throw Error('lock blob mismatch');
 const plans=Object.entries(JSON.parse(lockBytes).packages).filter(([,p])=>p.resolved).map(([k,p])=>plan(k,p));if(plans.length!==198)throw Error('full package count');writeNew('source-plan.json',plans);
 if(mode==='selftest'){
  for(const p of plans)assert.equal(redirect(p,'https://cdn.npmmirror.com'+p.expected_path,302,0),'https://cdn.npmmirror.com'+p.expected_path);
  const p=plans[0];let negative=0;for(const [u,s,h]of [['//cdn.npmmirror.com'+p.expected_path,302,0],['http://cdn.npmmirror.com'+p.expected_path,302,0],['https://cdn.npmmirror.com.evil'+p.expected_path,302,0],['https://user:pw@cdn.npmmirror.com'+p.expected_path,302,0],['https://cdn.npmmirror.com:444'+p.expected_path,302,0],['https://cdn.npmmirror.com'+p.expected_path+'?x=1',302,0],['https://cdn.npmmirror.com'+p.expected_path+'#x',302,0],['https://cdn.npmmirror.com'+p.expected_path,303,0],['https://cdn.npmmirror.com'+p.expected_path,302,1],['https://cdn.npmmirror.com/packages/other/1/x.tgz',302,0]]){assert.throws(()=>redirect(p,u,s,h));negative++;}
  writeNew('source-rules-selftest.json',{source:'SYNTHETIC no network',plans:plans.length,negative,scoped:plans.filter(p=>p.name.startsWith('@')).length});return;
 }
 const npmRoot=path.join(path.dirname(process.execPath),'node_modules/npm'),requireNpm=createRequire(path.join(npmRoot,'package.json')),cacache=requireNpm('cacache');
 for(const p of plans){remaining();const row={...p,utc:new Date().toISOString(),responses:[],integrity_verified:false,cache_written:false};records.push(row);try{
  let result=await get(p.origin,row,0);if(result.status!==200){result=await get(redirect(p,result.location,result.status,0),row,1);if(result.status!==200)throw Error('second response must be 200; no further redirect');}
  const bytes=result.bytes;row.bytes=bytes.length;row.sha256=crypto.createHash('sha256').update(bytes).digest('hex');if('sha512-'+crypto.createHash('sha512').update(bytes).digest('base64')!==p.integrity)throw Error('locked sha512 mismatch');row.integrity_verified=true;
  await cacache.put(path.join(d,'npm-cache/_cacache'),'make-fetch-happen:request-cache:'+p.origin,bytes,{integrity:p.integrity});row.cache_written=true;progress();
 }catch(e){row.error=String(e);progress();throw e;}}
 writeNew('dependency-retrievals.json',{script_sha256:identity.script_sha256,records});
 const args=[path.join(npmRoot,'bin/npm-cli.js'),'ci','--ignore-scripts','--no-audit','--no-fund','--offline'],before=crypto.createHash('sha256').update(lockBytes).digest('hex');
 const run=cp.spawnSync(process.execPath,args,{cwd:d,env:process.env,timeout:Math.min(5*60*1000,remaining()),maxBuffer:8*1024*1024});
 fs.writeFileSync(path.join(d,'npm-ci.stdout.bin'),run.stdout||Buffer.alloc(0),{flag:'wx'});fs.writeFileSync(path.join(d,'npm-ci.stderr.bin'),run.stderr||Buffer.alloc(0),{flag:'wx'});
 const after=crypto.createHash('sha256').update(fs.readFileSync(path.join(d,'package-lock.json'))).digest('hex');writeNew('npm-ci-result.json',{argv:args.slice(1),exit:run.status,error:run.error?String(run.error):null,signal:run.signal,lock_before:before,lock_after:after,elapsed_ms:Date.now()-started});
 if(run.error||run.status!==0||before!==after)throw Error('offline npm ci failed or changed lock');writeNew('preparation-complete.json',{packages:records.length,elapsed_ms:Date.now()-started,script_sha256:identity.script_sha256});
}catch(e){writeNew('preparation-failure.json',{error:String(e),elapsed_ms:Date.now()-started,records});console.error(String(e));process.exitCode=1;}})();
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
