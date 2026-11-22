# Fixed build-only CI entry. Never invoke the produced executable.
[CmdletBinding()]
param([string]$HostSourceSha, [string]$OfficialSha, [string]$EvidenceRoot)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$script:Ledger = [Collections.Generic.List[object]]::new()
$script:Phase = 'INPUT'
$script:BuildAttempts = 0
$script:Toolchain = '1.95.0-x86_64-pc-windows-msvc'
$script:LockHash = '49fe417bcafe3f69f1f6636e23274f90d37e75e388127a76888c39001a4f8117'

function Write-PHostJson($Path, $Value) {
    [IO.File]::WriteAllText($Path, (ConvertTo-Json -InputObject $Value -Depth 16), [Text.UTF8Encoding]::new($false))
}
function Get-PHostHash([string]$Path) {
    (Get-FileHash -LiteralPath $Path -Algorithm SHA256 -ErrorAction Stop).Hash.ToLowerInvariant()
}
function Assert-PHostHash([string]$Path, [string]$Expected) {
    if ((Get-PHostHash $Path) -cne $Expected) { throw 'P_HOST_HASH_REJECTED' }
}
function Assert-PHostInputs([string]$HostSha, [string]$Official) {
    if ($HostSha -cne '94092300d30eb3a96fffea04ea52c3d9522692ea' -or
        $Official -cne '092d3acd6bec3e3a14bdc7e7a2810ab628ab759d') { throw 'P_HOST_FIXED_INPUT_REQUIRED' }
}
function Assert-PHostCargoSources([string]$OfficialDirectory, [string]$CargoHome) {
    $allowed=Join-Path $OfficialDirectory '.cargo/config.toml'
    Assert-PHostHash $allowed 'b8ae1cea341beb2d4a3c8fb81f97a96f4aee1fd53f769c57f140dfe949806a80'
    # Cargo also searches cwd ancestors. Refuse unexpected config without reading it.
    for ($directory=Get-Item -LiteralPath $OfficialDirectory; $null -ne $directory; $directory=$directory.Parent) {
        foreach ($relative in @('.cargo/config','.cargo/config.toml')) {
            $candidate=Join-Path $directory.FullName $relative
            if ((Test-Path -LiteralPath $candidate) -and [IO.Path]::GetFullPath($candidate) -cne [IO.Path]::GetFullPath($allowed)) { throw 'P_HOST_UNEXPECTED_CARGO_CONFIG' }
        }
    }
    foreach ($name in @('config','config.toml')) {
        if (Test-Path -LiteralPath (Join-Path $CargoHome $name)) { throw 'P_HOST_UNEXPECTED_CARGO_CONFIG' }
    }
}
function Get-PHostEnvironment([Collections.IDictionary]$SystemValues, [Collections.IDictionary]$VcValues,
    [string]$Work, [string]$RustupHome) {
    # No inherited token/proxy/config/credential environment; include only these names.
    $result = @{}
    foreach ($key in @('SystemRoot','windir','ComSpec','PATHEXT','Path','ProgramFiles','ProgramFiles(x86)','ProgramW6432','NUMBER_OF_PROCESSORS','PROCESSOR_ARCHITECTURE')) {
        if ($SystemValues.Contains($key) -and $SystemValues[$key]) { $result[$key] = [string]$SystemValues[$key] }
    }
    foreach ($key in @('INCLUDE','LIB','LIBPATH','PATH','UCRTVersion','UniversalCRTSdkDir','VCINSTALLDIR','VCToolsInstallDir','WindowsLibPath','WindowsSdkBinPath','WindowsSdkDir','WindowsSDKLibVersion','WindowsSDKVersion')) {
        if ($VcValues.Contains($key) -and $VcValues[$key]) { $result[$key] = [string]$VcValues[$key] }
    }
    $result.TEMP = Join-Path $Work 'tmp'; $result.TMP = $result.TEMP
    $result.USERPROFILE = Join-Path $Work 'profile'; $result.HOME = $result.USERPROFILE
    $result.APPDATA = Join-Path $result.USERPROFILE 'AppData/Roaming'
    $result.LOCALAPPDATA = Join-Path $result.USERPROFILE 'AppData/Local'
    $result.CARGO_HOME = Join-Path $Work 'cargo'
    $result.CARGO_TARGET_DIR = Join-Path $Work 'target'
    $result.RUSTUP_HOME = $RustupHome
    $result.RUSTUP_AUTO_INSTALL = '0'; $result.CARGO_BUILD_JOBS = '2'
    $result.CARGO_NET_RETRY = '0'; $result.CARGO_HTTP_TIMEOUT = '60'; $result.CARGO_TERM_COLOR = 'never'
    $result.RUSTUP_DOWNLOAD_TIMEOUT = '60'; $result.RUSTUP_MAX_RETRIES = '0'
    $result.GIT_CONFIG_GLOBAL = 'NUL'; $result.GIT_CONFIG_NOSYSTEM = '1'; $result.GIT_TERMINAL_PROMPT = '0'
    $result.CI = 'true'
    return $result
}
function Invoke-PHostTool([string]$Label, [string]$Exe, [string[]]$Arguments,
    [string]$Directory, [Collections.IDictionary]$Environment, [int]$TimeoutSeconds,
    [switch]$PrivateOutput) {
    if ($Label -ceq 'cargo-build' -and $script:BuildAttempts -ne 0) { throw 'P_HOST_BUILD_ALREADY_ATTEMPTED' }
    $number = $script:Ledger.Count + 1
    $prefix = Join-Path $EvidenceRoot ('logs/{0:D2}-{1}' -f $number,$Label)
    $entry = [ordered]@{number=$number; phase=$script:Phase; label=$Label; executable=$Exe;
        executable_sha256=(Get-PHostHash $Exe); argv=$Arguments; started_utc=[DateTimeOffset]::UtcNow.ToString('o');
        timeout_seconds=$TimeoutSeconds; exit_code=$null; timed_out=$false; status='STARTING'}
    $info = [Diagnostics.ProcessStartInfo]::new(); $info.FileName=$Exe; $info.WorkingDirectory=$Directory
    $info.UseShellExecute=$false; $info.CreateNoWindow=$true
    $info.RedirectStandardOutput=$true; $info.RedirectStandardError=$true
    $info.StandardOutputEncoding=[Text.UTF8Encoding]::new($false); $info.StandardErrorEncoding=[Text.UTF8Encoding]::new($false)
    $info.Environment.Clear()
    foreach ($key in $Environment.Keys) { $info.Environment[$key]=[string]$Environment[$key] }
    foreach ($argument in $Arguments) { $info.ArgumentList.Add($argument) }
    $process=[Diagnostics.Process]::new(); $process.StartInfo=$info; $watch=[Diagnostics.Stopwatch]::StartNew()
    $stdout=''; $stderr=''; $started=$false; $drained=$false; $outFile=$null; $errFile=$null
    try {
        if (-not $process.Start()) { throw 'P_HOST_PROCESS_START_FAILED' }; $started=$true
        if ($Label -ceq 'cargo-build') { $script:BuildAttempts++ }
        $entry.pid=$process.Id; $entry.status='STARTED'
        $script:Ledger.Add($entry); Write-PHostJson (Join-Path $EvidenceRoot 'commands.json') @($script:Ledger.ToArray())
        if ($PrivateOutput) {
            $outTask=$process.StandardOutput.ReadToEndAsync(); $errTask=$process.StandardError.ReadToEndAsync()
        } else {
            # Stream logs during the command, so a job interruption does not erase all output.
            $outFile=[IO.File]::Open("$prefix.stdout.log",[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::Read)
            $errFile=[IO.File]::Open("$prefix.stderr.log",[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::Read)
            $outTask=$process.StandardOutput.BaseStream.CopyToAsync($outFile)
            $errTask=$process.StandardError.BaseStream.CopyToAsync($errFile)
        }
        if (-not $process.WaitForExit($TimeoutSeconds*1000)) {
            $entry.timed_out=$true
            # Kill via the retained process object, including its children, not a guessed PID.
            $process.Kill($true)
            if (-not $process.WaitForExit(10000)) { throw 'P_HOST_RECLAIM_UNKNOWN' }
        }
        $entry.exit_code=$process.ExitCode
        if (-not $outTask.Wait(10000) -or -not $errTask.Wait(10000)) { throw 'P_HOST_LOG_DRAIN_UNKNOWN' }
        if ($PrivateOutput) {
            $stdout=$outTask.GetAwaiter().GetResult(); $stderr=$errTask.GetAwaiter().GetResult()
        } else {
            $null=$outTask.GetAwaiter().GetResult(); $null=$errTask.GetAwaiter().GetResult()
        }
        $drained=$true
        $entry.status=if ($entry.timed_out) {'TIMEOUT'} elseif ($entry.exit_code -eq 0) {'EXITED_OK'} else {'EXITED_FAILED'}
    } finally {
        if ($started) {
            try {
                if (-not $process.HasExited) { $process.Kill($true); $null=$process.WaitForExit(10000) }
                $entry.reclaim=if($process.HasExited){'CONFIRMED'}else{'UNKNOWN'}
            } catch { $entry.reclaim='UNKNOWN' }
        } else { $entry.reclaim='NOT_STARTED' }
        if (-not $started) { $entry.status='START_FAILED'; $script:Ledger.Add($entry) }
        $entry.elapsed_seconds=[math]::Round($watch.Elapsed.TotalSeconds,3)
        $entry.logs_complete=$drained
        # MSVC environment capture is consumed only in memory, never dumped or hashed.
        if ($outFile) { $outFile.Dispose() }; if ($errFile) { $errFile.Dispose() }
        if ($PrivateOutput) {
            [IO.File]::WriteAllText("$prefix.stdout.log",'PRIVATE_ENVIRONMENT_CAPTURE_NOT_EXPORTED')
            [IO.File]::WriteAllText("$prefix.stderr.log",'PRIVATE_ENVIRONMENT_CAPTURE_NOT_EXPORTED')
        } else {
            if (-not (Test-Path -LiteralPath "$prefix.stdout.log")) { [IO.File]::WriteAllText("$prefix.stdout.log",'') }
            if (-not (Test-Path -LiteralPath "$prefix.stderr.log")) { [IO.File]::WriteAllText("$prefix.stderr.log",'') }
            if ($drained) { $stdout=[IO.File]::ReadAllText("$prefix.stdout.log"); $stderr=[IO.File]::ReadAllText("$prefix.stderr.log") }
        }
        $entry.stdout_log_sha256=Get-PHostHash "$prefix.stdout.log"; $entry.stderr_log_sha256=Get-PHostHash "$prefix.stderr.log"
        Write-PHostJson (Join-Path $EvidenceRoot 'commands.json') @($script:Ledger.ToArray())
        $process.Dispose()
    }
    if ($entry.reclaim -eq 'UNKNOWN') { throw 'P_HOST_RECLAIM_UNKNOWN' }
    if ($entry.timed_out -or $entry.exit_code -ne 0) { throw "P_HOST_TOOL_FAILED:$Label" }
    return [pscustomobject]@{Stdout=$stdout;Stderr=$stderr;ExitCode=$entry.exit_code}
}
function Get-PHostPayload([string]$Repo, [string]$Git, [Collections.IDictionary]$Environment) {
    $pins=[ordered]@{
        'host.rs'='2fc76f11fc71d83cdd617226d42337bb6e270169ecbd87247758979d4814b184'
        'bin-target.toml'='d18cca83e247c006b967666487c903a11952afe18c1240f2406dbd0071c1ffc4'
        'workspace-version.patch'='af985176df22c319b94b73803b6b6e2ca4c66e8f3e39860cdf6f3cc201fa7e54'
        'README.md'='9ec16478242fb71a544fe2d5108d3e9746f78722c7a01c5a5680a021a8c32fb2'
    }
    $record=@()
    foreach ($name in $pins.Keys) {
        $relative="tools/p-only-skip-host/$name"
        $blob=Invoke-PHostTool ('input-'+$name.Replace('.','-')) $Git @('show',"${HostSourceSha}:$relative") $Repo $Environment 30
        $path=Join-Path $EvidenceRoot "inputs/$name"
        [IO.File]::WriteAllText($path,$blob.Stdout,[Text.UTF8Encoding]::new($false)); Assert-PHostHash $path $pins[$name]
        $working=[IO.File]::ReadAllText((Join-Path $Repo $relative)).Replace("`r`n","`n")
        if ($working -cne $blob.Stdout) { throw 'P_HOST_WORKING_PAYLOAD_REJECTED' }
        $record+=@{file=$relative;canonical_sha256=$pins[$name]}
    }
    return $record
}
function Assert-PHostPe([string]$Path) {
    $stream=[IO.File]::OpenRead($Path); $reader=[IO.BinaryReader]::new($stream)
    try {
        if ($stream.Length -lt 64 -or $reader.ReadUInt16() -ne 0x5a4d) { throw 'P_HOST_PE_REJECTED' }
        $stream.Position=0x3c; $offset=$reader.ReadInt32()
        if ($offset -lt 64 -or $offset -gt $stream.Length-6) { throw 'P_HOST_PE_REJECTED' }
        $stream.Position=$offset
        if ($reader.ReadUInt32() -ne 0x4550 -or $reader.ReadUInt16() -ne 0x8664) { throw 'P_HOST_PE_REJECTED' }
    } finally { $reader.Dispose() }
}
function Complete-PHostEvidence([Collections.IDictionary]$Identity, [Collections.IDictionary]$Binding) {
    Write-PHostJson (Join-Path $EvidenceRoot 'identity.json') $Identity
    Write-PHostJson (Join-Path $EvidenceRoot 'source-binding.json') $Binding
    Write-PHostJson (Join-Path $EvidenceRoot 'commands.json') @($script:Ledger.ToArray())
    $files=@(Get-ChildItem -LiteralPath $EvidenceRoot -Recurse -File | Where-Object Name -ne 'manifest.json' | Sort-Object FullName | ForEach-Object {
        if ($_.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'P_HOST_EVIDENCE_REPARSE' }
        @{file=[IO.Path]::GetRelativePath($EvidenceRoot,$_.FullName).Replace('\','/');bytes=$_.Length;sha256=Get-PHostHash $_.FullName}
    })
    Write-PHostJson (Join-Path $EvidenceRoot 'manifest.json') @{schema='nova.p-host.files/1';files=$files}
}

# No test mode or executable injection. Tests extract pure functions via PowerShell AST.
$identity=[ordered]@{schema='nova.p-host.build/1';workflow_sha=$env:GITHUB_SHA;host_source_sha=$HostSourceSha;
    official_sha=$OfficialSha;run_id=$env:GITHUB_RUN_ID;attempt=$env:GITHUB_RUN_ATTEMPT;
    runner_image_os=$env:ImageOS;runner_image_version=$env:ImageVersion;
    started_utc=[DateTimeOffset]::UtcNow.ToString('o');status='NOT_STARTED';host_present=$false;host_executed=$false;exe=$null}
$binding=[ordered]@{}; $failure=$null; $ready=$false; $lock=$null; $beforeDefault=$null; $environment=$null
try {
    if ($PSVersionTable.PSVersion.Major -lt 7 -or -not $IsWindows -or $env:GITHUB_ACTIONS -cne 'true' -or
        $env:GITHUB_RUN_ID -cnotmatch '^[1-9][0-9]{0,19}$' -or $env:GITHUB_RUN_ATTEMPT -cnotmatch '^[1-9][0-9]{0,9}$' -or
        $env:GITHUB_SHA -cnotmatch '^[a-f0-9]{40}$') { throw 'P_HOST_RUN_CONTEXT_REJECTED' }
    $expected=[IO.Path]::GetFullPath((Join-Path $env:RUNNER_TEMP 'p-only-host-package'))
    if (-not $EvidenceRoot -or [IO.Path]::GetFullPath($EvidenceRoot) -cne $expected -or -not (Test-Path -LiteralPath $expected -PathType Container)) { throw 'P_HOST_EVIDENCE_ROOT_REJECTED' }
    for ($parent=Get-Item -LiteralPath $expected; $null -ne $parent; $parent=$parent.Parent) {
        if ($parent.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'P_HOST_EVIDENCE_REPARSE' }
    }
    foreach ($dir in @('logs','inputs')) { New-Item -ItemType Directory -Path (Join-Path $EvidenceRoot $dir) -ErrorAction Stop | Out-Null }
    $ready=$true
    Write-PHostJson (Join-Path $EvidenceRoot 'identity.json') $identity
    Assert-PHostInputs $HostSourceSha $OfficialSha
    $repo=[IO.Path]::GetFullPath($env:GITHUB_WORKSPACE)
    $work=Join-Path $env:RUNNER_TEMP "ph-$($env:GITHUB_RUN_ID)-$($env:GITHUB_RUN_ATTEMPT)"
    New-Item -ItemType Directory -Path $work -ErrorAction Stop | Out-Null
    $system=[Environment]::GetEnvironmentVariables('Process')
    $rustupHome=if($env:RUSTUP_HOME){$env:RUSTUP_HOME}else{Join-Path $env:USERPROFILE '.rustup'}
    $environment=Get-PHostEnvironment $system @{} $work $rustupHome
    foreach ($path in @($environment.TEMP,$environment.USERPROFILE,$environment.APPDATA,$environment.LOCALAPPDATA,$environment.CARGO_HOME)) { New-Item -ItemType Directory -Path $path -Force | Out-Null }
    Write-PHostJson (Join-Path $EvidenceRoot 'child-environment-keys.json') @($environment.Keys | Sort-Object)
    $git=(Get-Command git.exe -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
    $rustup=(Get-Command rustup.exe -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
    $cargo=(Get-Command cargo.exe -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
    $head=Invoke-PHostTool 'checkout-head' $git @('rev-parse','HEAD') $repo $environment 30
    if ($head.Stdout.Trim() -cne $env:GITHUB_SHA) { throw 'P_HOST_CHECKOUT_SHA_REJECTED' }
    $identity.checkout_head=$head.Stdout.Trim()
    Invoke-PHostTool 'ancestor' $git @('merge-base','--is-ancestor',$HostSourceSha,'HEAD') $repo $environment 30 | Out-Null
    foreach ($path in @('tools/p-only-skip-host','apps/desktop/src-tauri')) {
        Invoke-PHostTool ('source-diff-'+$script:Ledger.Count) $git @('diff','--exit-code',$HostSourceSha,'HEAD','--',$path) $repo $environment 30 | Out-Null
        Invoke-PHostTool ('working-diff-'+$script:Ledger.Count) $git @('diff','--exit-code','HEAD','--',$path) $repo $environment 30 | Out-Null
        $untracked=Invoke-PHostTool ('untracked-'+$script:Ledger.Count) $git @('ls-files','--others','--exclude-standard','--',$path) $repo $environment 30
        if ($untracked.Stdout.Length) { throw 'P_HOST_UNTRACKED_SOURCE_REJECTED' }
    }
    $binding.payload=@(Get-PHostPayload $repo $git $environment)
    $binding.workflow_sha256=Get-PHostHash (Join-Path $repo '.github/workflows/windows-ci.yml')
    $binding.script_sha256=Get-PHostHash $PSCommandPath
    $binding.child_environment_scope='WHITELIST_ONLY_NO_PLATFORM_TOKENS_OR_USER_CREDENTIAL_CONFIG'
    $script:Phase='OFFICIAL_SOURCE'
    $archive=Join-Path $work 'official.tar.gz'; $src=Join-Path $work 'src'
    $url="https://codeload.github.com/openai/codex/tar.gz/$OfficialSha"
    $curl=(Get-Command curl.exe -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
    Invoke-PHostTool 'official-archive' $curl @('--disable','--fail','--proto','=https','--retry','0','--connect-timeout','30','--max-time','180','--output',$archive,$url) $work $environment 190 | Out-Null
    $archiveHash='79c8ea8547e1dd9969f63ef6b76a4fb0d1a024ef36c0c627fbcd2ffb658d9312'
    Assert-PHostHash $archive $archiveHash; $binding.archive=@{url=$url;sha256=$archiveHash}
    # Extract only a hash-verified fixed archive; reject traversal, special files and unknown links.
    $extract=Join-Path $work 'extract.py'
    $pythonSource=@"
import pathlib,sys,tarfile,shutil
archive,out,commit=pathlib.Path(sys.argv[1]),pathlib.Path(sys.argv[2]),sys.argv[3]
out.mkdir(exist_ok=False)
with tarfile.open(archive,'r:gz') as tar:
 for m in tar:
  p=pathlib.PurePosixPath(m.name)
  if p.is_absolute() or '..' in p.parts or not p.parts or p.parts[0]!='codex-'+commit or any(':' in x for x in p.parts): raise RuntimeError('ARCHIVE_PATH_REJECTED')
  target=out.joinpath(*p.parts[1:])
  if m.isdir(): target.mkdir(parents=True,exist_ok=True)
  elif m.isfile():
   target.parent.mkdir(parents=True,exist_ok=True)
   with tar.extractfile(m) as src,target.open('xb') as dst: shutil.copyfileobj(src,dst)
  elif m.issym() and p.parts[1:]==('codex-rs','vendor','bubblewrap','LICENSE') and m.linkname=='COPYING': target.write_bytes(b'COPYING')
  else: raise RuntimeError('ARCHIVE_ENTRY_REJECTED')
print('FROZEN_ARCHIVE_EXTRACTED')
"@
    [IO.File]::WriteAllText($extract,$pythonSource,[Text.UTF8Encoding]::new($false))
    $python=(Get-Command python.exe -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
    Invoke-PHostTool 'extract' $python @('-I',$extract,$archive,$src,$OfficialSha) $work $environment 300 | Out-Null
    $official=Join-Path $src 'codex-rs'; $lock=Join-Path $official 'Cargo.lock'
    Assert-PHostHash $lock $script:LockHash; $binding.lock_before=Get-PHostHash $lock
    $binding.toolchain_sha256=Get-PHostHash (Join-Path $official 'rust-toolchain.toml')
    $binding.cargo_config_sha256=Get-PHostHash (Join-Path $official '.cargo/config.toml')
    Assert-PHostCargoSources $official $environment.CARGO_HOME
    $appManifest=Join-Path $official 'app-server/Cargo.toml'
    Assert-PHostHash $appManifest 'b42b0fcb26e284d66c32411f8995512337e3a826a8aefcd2ae6bad7e5e708b0c'
    Invoke-PHostTool 'version-patch-check' $git @('apply','--check','--unidiff-zero',(Join-Path $EvidenceRoot 'inputs/workspace-version.patch')) $src $environment 30 | Out-Null
    Invoke-PHostTool 'version-patch' $git @('apply','--unidiff-zero',(Join-Path $EvidenceRoot 'inputs/workspace-version.patch')) $src $environment 30 | Out-Null
    Assert-PHostHash (Join-Path $official 'Cargo.toml') '26a713b6714637e008a30b0bf17fb03f1111c09f3d944857e2c4c1b050a20779'
    [IO.File]::WriteAllBytes($appManifest,([byte[]]([IO.File]::ReadAllBytes($appManifest)+[IO.File]::ReadAllBytes((Join-Path $EvidenceRoot 'inputs/bin-target.toml')))))
    Assert-PHostHash $appManifest '9e6143fe93a8c5fdd6154f44da330d650d99f24042dc634728e40af285c17f76'
    $entry=Join-Path $official 'app-server/src/bin/nova_p_only_skip_host.rs'
    if (Test-Path -LiteralPath $entry) { throw 'P_HOST_ENTRY_EXISTS' }
    [IO.File]::WriteAllBytes($entry,[IO.File]::ReadAllBytes((Join-Path $EvidenceRoot 'inputs/host.rs')))
    $binding.patched_workspace_sha256=Get-PHostHash (Join-Path $official 'Cargo.toml')
    $binding.patched_app_manifest_sha256=Get-PHostHash $appManifest
    $binding.entry_sha256=Get-PHostHash $entry
    $script:Phase='TOOLS'
    $rustupVersion=Invoke-PHostTool 'rustup-version' $rustup @('--version') $work $environment 30
    $identity.rustup=$rustupVersion.Stdout.Trim()
    $beforeDefault=Invoke-PHostTool 'default-before' $rustup @('show','active-toolchain') $work $environment 30
    $identity.default_before=$beforeDefault.Stdout.Trim()
    Invoke-PHostTool 'install-1-95' $rustup @('toolchain','install',$script:Toolchain,'--profile','minimal','--component','clippy','--component','rustfmt','--component','rust-src','--no-self-update') $work $environment 600 | Out-Null
    $defaultAfter=Invoke-PHostTool 'default-after' $rustup @('show','active-toolchain') $work $environment 30
    $identity.default_after=$defaultAfter.Stdout.Trim()
    if ($identity.default_before -cne $identity.default_after) { throw 'P_HOST_DEFAULT_TOOLCHAIN_CHANGED' }
    $installed=Invoke-PHostTool 'components' $rustup @('component','list','--installed','--toolchain',$script:Toolchain) $work $environment 30
    foreach ($component in @('rustc','cargo','rust-std','clippy','rustfmt')) {
        if ($installed.Stdout -notmatch "(?m)^$component-x86_64-pc-windows-msvc\r?$" ) { throw 'P_HOST_COMPONENT_MISSING' }
    }
    if ($installed.Stdout -notmatch '(?m)^rust-src\r?$') { throw 'P_HOST_COMPONENT_MISSING' }
    $version=Invoke-PHostTool 'rustc-version' $rustup @('run',$script:Toolchain,'rustc','--version','--verbose') $work $environment 30
    if ($version.Stdout -notmatch '^rustc 1\.95\.0 ' -or $version.Stdout -notmatch '(?m)^host: x86_64-pc-windows-msvc\r?$') { throw 'P_HOST_TOOLCHAIN_REJECTED' }
    $identity.rustc=$version.Stdout.Trim()
    $cargoVersion=Invoke-PHostTool 'cargo-version' $cargo @("+$($script:Toolchain)",'--version') $work $environment 30
    if ($cargoVersion.Stdout -notmatch '^cargo 1\.95\.0 ') { throw 'P_HOST_CARGO_VERSION_REJECTED' }
    $identity.cargo=$cargoVersion.Stdout.Trim()
    $vswhere=Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
    $vs=Invoke-PHostTool 'vswhere' $vswhere @('-latest','-products','*','-requires','Microsoft.VisualStudio.Component.VC.Tools.x86.x64','-property','installationPath') $work $environment 30
    $vsRoot=$vs.Stdout.Trim()
    if (-not $vsRoot -or $vsRoot.Contains("`n") -or -not (Test-Path 'HKLM:\SOFTWARE\Microsoft\Windows Kits\Installed Roots')) { throw 'P_HOST_MSVC_SDK_MISSING' }
    $batch=Join-Path $work 'vc.cmd'; $vsCmd=Join-Path $vsRoot 'Common7/Tools/VsDevCmd.bat'
    if (-not (Test-Path -LiteralPath $vsCmd)) { throw 'P_HOST_MSVC_MISSING' }
    [IO.File]::WriteAllText($batch,"@echo off`r`ncall `"$vsCmd`" -no_logo -arch=x64 -host_arch=x64 >nul`r`nif errorlevel 1 exit /b 1`r`nset`r`n",[Text.Encoding]::ASCII)
    $vc=Invoke-PHostTool 'msvc-environment' (Join-Path $env:SystemRoot 'System32/cmd.exe') @('/d','/c',$batch) $work $environment 60 -PrivateOutput
    $vcValues=@{}
    foreach ($line in $vc.Stdout -split "`r?`n") { if ($line -match '^([^=]+)=(.*)$') { $vcValues[$Matches[1]]=$Matches[2] } }
    if (-not $vcValues.VCToolsInstallDir -or -not $vcValues.INCLUDE -or -not $vcValues.LIB) { throw 'P_HOST_MSVC_ENV_MISSING' }
    $environment=Get-PHostEnvironment $system $vcValues $work $rustupHome
    $environment.RUSTUP_TOOLCHAIN=$script:Toolchain
    $cmake=Join-Path $vsRoot 'Common7/IDE/CommonExtensions/Microsoft/CMake/CMake/bin'
    if (Test-Path -LiteralPath (Join-Path $cmake 'cmake.exe')) { $environment.PATH=$cmake+';'+$environment.PATH }
    Write-PHostJson (Join-Path $EvidenceRoot 'child-environment-keys.json') @($environment.Keys | Sort-Object)
    $script:Phase='BUILD'
    Assert-PHostCargoSources $official $environment.CARGO_HOME
    $buildArgs=@("+$($script:Toolchain)",'build','--locked','--target','x86_64-pc-windows-msvc','-p','codex-app-server','--bin','nova-p-only-skip-host','--config','net.retry=0')
    $identity.build_argv=$buildArgs
    Invoke-PHostTool 'cargo-build' $cargo $buildArgs $official $environment 5400 | Out-Null
    Assert-PHostHash $lock $script:LockHash
    $script:Phase='ARTIFACT'
    $exe=Join-Path $environment.CARGO_TARGET_DIR 'x86_64-pc-windows-msvc/debug/nova-p-only-skip-host.exe'
    Assert-PHostPe $exe
    $bin=Join-Path $EvidenceRoot 'bin'; New-Item -ItemType Directory -Path $bin -ErrorAction Stop | Out-Null
    Copy-Item -LiteralPath $exe -Destination (Join-Path $bin 'nova-p-only-skip-host.exe') -ErrorAction Stop
    $identity.exe=@{file='bin/nova-p-only-skip-host.exe';bytes=(Get-Item -LiteralPath $exe).Length;sha256=Get-PHostHash $exe;pe_machine='0x8664';signing_requested=$false;authenticode='NOT_CHECKED'}
    $identity.host_present=$true; $identity.status='BUILD_SUCCEEDED_NOT_EXECUTED'
} catch {
    $failure='P_HOST_BUILD_STOPPED'; $identity.status='FAILED'; $identity.failure_phase=$script:Phase
    # Only fixed local rule codes, never arbitrary exception text/HTTP bodies/environment.
    $message=$_.Exception.Message
    $identity.failure_code=if($message -cmatch '^P_HOST_[A-Z0-9_]+(?::[a-z0-9-]+)?$'){$message}else{'P_HOST_LOCAL_EXCEPTION'}
    $identity.exception_type=$_.Exception.GetType().FullName
    $identity.host_present=$false; $identity.exe=$null
} finally {
    if ($ready) {
        if ($lock -and (Test-Path -LiteralPath $lock)) {
            $binding.lock_after=Get-PHostHash $lock
            if ($binding.lock_after -cne $script:LockHash) { $failure='P_HOST_LOCK_DRIFT';$identity.status='FAILED';$identity.failure_code=$failure;$identity.host_present=$false;$identity.exe=$null }
        }
        if ($failure) {
            # Only our own temporary package member, never source/cache/user files.
            $staged=Join-Path $EvidenceRoot 'bin/nova-p-only-skip-host.exe'
            if (Test-Path -LiteralPath $staged -PathType Leaf) { [IO.File]::Delete($staged) }
        }
        $identity.build_attempts=$script:BuildAttempts; $identity.finished_utc=[DateTimeOffset]::UtcNow.ToString('o')
        Complete-PHostEvidence $identity $binding
    }
}
if ($failure) { throw $failure }
