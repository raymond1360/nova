# Fixed future preparation entry. This development pack does not execute it.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
if ($args.Count -ne 0) { throw 'No arguments accepted' }
$inputRoot = 'D:\NovaM0BInput'
$runRoot = 'C:\NovaM0B\run'
$policy = Get-Content -LiteralPath "$inputRoot\resource-policy.json" -Raw | ConvertFrom-Json
if ($policy.schema -cne 'nova-m0b-resource-policy-v1' -or
    $policy.prepared -ne $true -or $policy.execution_authorized -ne $true -or
    $null -eq $policy.facility_receipt -or [string]::IsNullOrEmpty($policy.probe_sid)) {
    throw 'BLOCKED: approved resource and facility receipts not prepared'
}
# No inheritance from user resources. No downloads, account creation or observer actions.
$sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
if ($sid -cne $policy.probe_sid) { throw 'BLOCKED: fixed standard probe SID required' }
if (Test-Path -LiteralPath $runRoot) { throw 'Refuse existing preparation root' }
$names = @('opencode','rg','git','git_children','plugin_closure','fixture','node')
foreach ($name in $names) {
    if ($null -eq $policy.resources.$name) { throw "BLOCKED: resource $name absent" }
}
# Closed input record table: relative paths may not redirect outside the fixed ISO.
$records = $policy.resources.fixture.files
if ($null -eq $records -or $records.Count -eq 0) { throw 'BLOCKED: fixture file manifest absent' }
foreach ($f in $records) {
    if ($f.path -notmatch '^[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)*$' -or
        $f.path -match '(^|/)\.\.(/|$)' -or $f.sha256 -cnotmatch '^[0-9a-f]{64}$') {
        throw 'BLOCKED: non-canonical fixture record'
    }
    $src = Join-Path "$inputRoot\fixture" $f.path
    $item = Get-Item -LiteralPath $src -Force
    if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $item.Length -ne $f.size -or
        (Get-FileHash -LiteralPath $src -Algorithm SHA256).Hash.ToLowerInvariant() -cne $f.sha256) {
        throw 'BLOCKED: fixture identity mismatch'
    }
}
# Preparation does not invoke Git, OpenCode, rg, observers or networking.
New-Item -ItemType Directory -Path "$runRoot\project" -ErrorAction Stop | Out-Null
foreach ($f in $records) {
    $dst = Join-Path "$runRoot\project" $f.path
    New-Item -ItemType Directory -Path ([IO.Path]::GetDirectoryName($dst)) -Force | Out-Null
    [IO.File]::Copy((Join-Path "$inputRoot\fixture" $f.path), $dst, $false)
}
if ($policy.root_commit -cnotmatch '^[0-9a-f]{40}$') { throw 'BLOCKED: root commit identity absent' }
$gitMarker = "$runRoot\project\.git\opencode"
if (Test-Path -LiteralPath $gitMarker) { throw 'Refuse preexisting marker' }
[IO.File]::WriteAllBytes($gitMarker, [Text.Encoding]::ASCII.GetBytes($policy.root_commit))
[IO.File]::SetAttributes($gitMarker, [IO.FileAttributes]::Archive)
foreach ($item in @(Get-Item -LiteralPath "$runRoot\project" -Force) +
    @(Get-ChildItem -LiteralPath "$runRoot\project" -Recurse -Force)) {
    $acl = Get-Acl -LiteralPath $item.FullName
    $owner = $acl.GetOwner([Security.Principal.SecurityIdentifier]).Value
    if ($owner -cne $sid) { throw 'BLOCKED: copied tree owner mismatch; no runtime repair' }
}
# Record copying only, without granting native acceptance.
New-Item -ItemType Directory -Path "$runRoot\evidence" -ErrorAction Stop | Out-Null
$receipt = @{schema='nova-m0b-preparation-v1'; probe_sid=$sid;
    policy_sha256=(Get-FileHash -LiteralPath "$inputRoot\resource-policy.json" -Algorithm SHA256).Hash.ToLowerInvariant();
    root_commit=$policy.root_commit; git_preflight='NOT_RUN'; native_job='NOT_RUN';
    execution_authorized=$false; prepared_files=$records.Count}
$f=[IO.File]::Open("$runRoot\evidence\preparation.json",[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::Read)
try { $b=[Text.Encoding]::UTF8.GetBytes(($receipt|ConvertTo-Json -Compress));$f.Write($b,0,$b.Length);$f.Flush($true) } finally {$f.Dispose()}
Write-Output 'Copy recorded; no Git preflight, target or capture started; execution remains separately gated.'
