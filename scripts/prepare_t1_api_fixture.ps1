#requires -Version 7.0
param(
 [Parameter(Mandatory=$true,ParameterSetName='Fixture')][string]$RunDirectory,
 [Parameter(Mandatory=$true,ParameterSetName='Dependencies')][string]$VerifierDeps,
 [Parameter(Mandatory=$true,ParameterSetName='Dependencies')][string]$NodePath
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if ($PSCmdlet.ParameterSetName -eq 'Dependencies') {
 if (-not [IO.Path]::IsPathRooted($NodePath) -or (& $NodePath --version) -cne 'v24.14.0' -or
     (Get-FileHash -LiteralPath $NodePath -Algorithm SHA256).Hash.ToLowerInvariant() -cne '63c259c81e5d472b5f11c8d506070130cb04a1ecf84b80377a34ed6ec9048088') { throw 'Pinned absolute official Windows x64 Node required' }
 if (Test-Path -LiteralPath $VerifierDeps) { throw 'Fresh isolated verifier directory required' }
 New-Item -ItemType Directory -Path $VerifierDeps | Out-Null
 Push-Location $root
 try {
  # Binary-safe export of the reviewed LF Git blobs. No working-tree EOL conversion.
  $export = @'
const fs=require('node:fs'),cp=require('node:child_process'),path=require('node:path');
const d=process.argv[1];
for(const f of ['package.json','package-lock.json'])fs.writeFileSync(path.join(d,f),cp.execFileSync('git',['show','7788e9bfd6d7fc6baccce503b0ad5d873790d026:apps/desktop/'+f]));
'@
  & $NodePath -e $export $VerifierDeps
  if ($LASTEXITCODE -ne 0) { throw 'Blob export failed' }
 } finally { Pop-Location }
 $prepare = @'
const fs=require('node:fs'),path=require('node:path'),https=require('node:https'),crypto=require('node:crypto'),cp=require('node:child_process');
const {createRequire}=require('node:module');
const d=path.resolve(process.argv[1]);const lockBytes=fs.readFileSync(path.join(d,'package-lock.json'));
if(crypto.createHash('sha256').update(lockBytes).digest('hex')!=='097850e97d14a6280dbc2ca91b411ecc4aa6bebf327a5a97d8d647a07fe5b68a')throw Error('lock blob mismatch');
const lock=JSON.parse(lockBytes);const npmRoot=path.join(path.dirname(process.execPath),'node_modules/npm');
const req=createRequire(path.join(npmRoot,'package.json'));const cacache=req('cacache');
const cache=path.join(d,'npm-cache');const records=[];
function get(url){return new Promise((ok,no)=>{const r=https.get(url,{timeout:30000},res=>{
 if(res.statusCode!==200){res.resume();return no(Error('Exact URL failed; redirects NOT AUTHORIZED: '+res.statusCode+' '+(res.headers.location||'')));}
 let size=0;const chunks=[];res.on('data',b=>{size+=b.length;if(size>50*1024*1024){res.destroy(Error('package bound'));}else chunks.push(b);});res.on('end',()=>ok(Buffer.concat(chunks)));res.on('error',no);
});r.on('timeout',()=>r.destroy(Error('timeout')));r.on('error',no);});}
(async()=>{try{
 const packages=new Map();for(const p of Object.values(lock.packages).filter(p=>p.resolved)){
  if(packages.has(p.resolved)&&packages.get(p.resolved)!==p.integrity)throw Error('ambiguous locked integrity');packages.set(p.resolved,p.integrity);
 }
 for(const [url,integrity]of packages){
  const u=new URL(url);if(u.origin!=='https://registry.npmmirror.com'||u.username||u.password||u.search||u.hash)throw Error('unapproved locked URL');
  const bytes=await get(url);const [alg,value]=integrity.split('-');if(!['sha512','sha256','sha1'].includes(alg)||crypto.createHash(alg).update(bytes).digest('base64')!==value)throw Error('integrity mismatch');
  await cacache.put(path.join(cache,'_cacache'),'make-fetch-happen:request-cache:'+url,bytes,{integrity});
  records.push({url,integrity,bytes:bytes.length,sha256:crypto.createHash('sha256').update(bytes).digest('hex')});
 }
 fs.writeFileSync(path.join(d,'dependency-retrievals.json'),JSON.stringify(records,null,2),{flag:'wx'});
 const user=path.join(d,'npm-user-empty.npmrc'),global=path.join(d,'npm-global-empty.npmrc');fs.writeFileSync(user,'',{flag:'wx'});fs.writeFileSync(global,'',{flag:'wx'});
 const env={};for(const k of ['SystemRoot','SYSTEMROOT','WINDIR','TEMP','TMP','COMSPEC','ComSpec'])if(process.env[k])env[k]=process.env[k];
 Object.assign(env,{PATH:path.dirname(process.execPath),NPM_CONFIG_USERCONFIG:user,NPM_CONFIG_GLOBALCONFIG:global,NPM_CONFIG_CACHE:cache,NPM_CONFIG_REGISTRY:'https://registry.npmmirror.com',NPM_CONFIG_IGNORE_SCRIPTS:'true',NPM_CONFIG_AUDIT:'false',NPM_CONFIG_FUND:'false',NPM_CONFIG_OFFLINE:'true'});
 const r=cp.spawnSync(process.execPath,[path.join(npmRoot,'bin/npm-cli.js'),'ci','--ignore-scripts','--no-audit','--no-fund','--offline'],{cwd:d,env,stdio:'inherit'});
 if(r.error||r.status!==0)throw Error('offline npm ci failed');
 if(!fs.readFileSync(path.join(d,'package-lock.json')).equals(lockBytes))throw Error('lock modified');
}catch(e){fs.writeFileSync(path.join(d,'preparation-failure.json'),JSON.stringify({error:String(e),verified:records},null,2),{flag:'wx'});throw e;}})().catch(e=>{console.error(String(e));process.exitCode=1;});
'@
 & $NodePath -e $prepare $VerifierDeps
 if ($LASTEXITCODE -ne 0) { throw 'Verifier dependency preparation BLOCKED; do not change sources or enable scripts' }
 return
}
if (Test-Path -LiteralPath $RunDirectory) { throw 'Refusing existing run directory' }
$source = Join-Path $root 'tools/t1-api-eval/fixtures/Logo.tsx'
if ((Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash.ToLowerInvariant() -cne '0821ae3bbb7e9d2085e9ae9031b4b7744bc191c84a6b7dda6b618b7c1b6ed58a') { throw 'Fixture identity mismatch' }
$project = Join-Path $RunDirectory 'project/src/components/Logo'
New-Item -ItemType Directory -Path $project | Out-Null
$target = Join-Path $project 'Logo.tsx'
$bytes = [IO.File]::ReadAllBytes($source)
$stream = [IO.File]::Open($target, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::Read)
try { $stream.Write($bytes,0,$bytes.Length); $stream.Flush($true) } finally { $stream.Dispose() }
if ((Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash.ToLowerInvariant() -cne '0821ae3bbb7e9d2085e9ae9031b4b7744bc191c84a6b7dda6b618b7c1b6ed58a') { throw 'Fixture readback mismatch' }
Write-Output 'Prepared fixed Logo bytes only; no dependencies, runtime, database or Agent started.'
