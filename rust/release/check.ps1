#Requires -Version 7
<#
.SYNOPSIS
Stream-test and install a published Rubylight release on the Radeon host.

.DESCRIPTION
Pushing a tag builds, tests, packages and publishes a release on GitHub
Actions (.github/workflows/rust-windows.yml). This script is the part only
the release workstation can do, run when its installed host is idle:
1. Downloads the release's installer and portable ZIP and checks them against
   SHA256SUMS.
2. Streams H.264, HEVC, HEVC VRR, AV1 and PyroWave (SDR and HDR 4:4:4)
   through the packaged host with the independent moonlight-common-c client,
   plus HEVC with keyframe requests from the client while the picture moves
   at the stream rate, HEVC at 1440p120 with 80 Mbps, and the same launched
   at 60 as Artemis's Warp does (both skipped below a 120 Hz display), then
   runs the protocol checks (e2e.py, protocol.py).
3. The display self-test as SYSTEM and a quiet install over the running host
   (elevated.ps1): directly from an elevated shell, otherwise through the task
   elevation.ps1 installs, otherwise after one UAC prompt.
4. Uploads the results to the release as VALIDATION.json (validation.py).
The laptop's real-client smoke test (smoke.ps1) runs after the install and
adds REAL-CLIENT.json.

.EXAMPLE
pwsh rust/release/check.ps1                  # the newest release
.EXAMPLE
pwsh rust/release/check.ps1 v2.2.2 -NoInstall -NoUpload
#>
param(
    # Defaults to the newest published release.
    [string] $Version,
    # Not under %LOCALAPPDATA%: a packaged (MSIX) app such as the Claude
    # desktop app sees its own copy of that folder, so the SYSTEM self-test
    # could not find a package unpacked there.
    [string] $Work = 'C:\src\butterpollo-release',
    # Machine settings, dot-sourced: the Rust build environment (PATH,
    # PyroWave, ...) and BUTTERPOLLO_TEST_PYTHON.
    [string] $Settings = (Join-Path $Work 'settings.ps1'),
    # Stream cases to leave out, e.g. 'pyrowave','pyrowave-hdr-444' when a
    # shared machine cannot sustain them; recorded in VALIDATION.json.
    [string[]] $SkipStreams = @(),
    # A smoke.ps1 result from the laptop to include; smoke.ps1 normally
    # uploads its own REAL-CLIENT.json after this check installed the release.
    [string] $RealClient,
    # The shared BUILD-LOCK and GPU-LOCK of parallel sessions on this machine
    # (check_streams.ps1); without build-lock.ps1 there the streams only wait
    # for a quiet CPU. Take no lock around this script: it takes them itself.
    [string] $LockDirectory = 'C:\src\butterpollo-release',
    # The caller holds GPU-LOCK for this check under its own name. A holder
    # among this process's parents (tools/hw/run.py) is found without it.
    [switch] $GpuLockHeld,
    [switch] $NoInstall,
    [switch] $NoUpload
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$repo = 'RamazanKara/Rubylight'
$started = Get-Date
function Step($name) { Write-Host ('[{0:mm\:ss}] {1}' -f ((Get-Date) - $started), $name) -ForegroundColor Cyan }

if (Test-Path $Settings) { . $Settings }
$python = if ($env:BUTTERPOLLO_TEST_PYTHON) { $env:BUTTERPOLLO_TEST_PYTHON } else { 'python' }
$tools = $PSScriptRoot
$checkout = (Resolve-Path "$PSScriptRoot\..\..").Path
if (-not $Version) { $Version = gh release list -R $repo --exclude-drafts --limit 1 --json tagName --jq '.[0].tagName' }
$tag = $Version
$Version = $Version -replace '^v', ''
# The layout elevated-task.ps1 expects: <Work>\<version>\release\...
$run = Join-Path $Work $Version
$out = "$run\release"
$qa = "$run\qa"
# Kept between releases, so the receivers and probes only rebuild what changed.
$fixtures = Join-Path $Work 'fixtures'
$package = "$out\butterpollo-rust-release"
$installer = "$out\rubylight-setup-$Version.exe"
$zip = "$out\rubylight-$Version-windows-x64.zip"

# The stream checks refuse to run beside a busy installed host; say so before
# minutes of downloads and fixture builds.
try {
    & $python -c "import sys; sys.path.insert(0, r'$tools'); import e2e_host; e2e_host.installed_idle()"
} catch {
    throw 'The installed host is streaming or runs an app started from Moonlight. Disconnect and close it, then run the check again.'
}

Step "download $Version"
Remove-Item -Recurse -Force $out -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $out, $qa | Out-Null
# Releases before rc.30 carry only the Butterpollo names.
gh release download $tag -R $repo -D $out -p 'rubylight-setup-*.exe' -p 'rubylight-*-windows-x64.zip' -p 'butterpollo-setup-*.exe' -p 'butterpollo-rust-*-windows-x64.zip' -p SHA256SUMS
if (-not (Test-Path $installer)) {
    $installer = "$out\butterpollo-setup-$Version.exe"
    $zip = "$out\butterpollo-rust-$Version-windows-x64.zip"
}
foreach ($line in Get-Content "$out\SHA256SUMS") {
    $hash, $name = $line -split '\s+', 2
    if ((Get-FileHash "$out\$name" -Algorithm SHA256).Hash -ne $hash) { throw "$name does not match SHA256SUMS" }
}
Expand-Archive $zip -DestinationPath $out

. "$PSScriptRoot\check_streams.ps1"
Initialize-CheckLocks $LockDirectory -GpuLockHeld:$GpuLockHeld

Step 'test receivers and probes'
git -C $checkout submodule update --init --recursive --depth 1 third-party/moonlight-common-c
# Builds hold BUILD-LOCK only; GPU-LOCK is for the streams.
$held = Enter-CheckBuildLock
try {
    & "$checkout\rust\tests\build-moonlight-client.ps1" -ArtifactDirectory $fixtures *> "$qa\receiver-build.log"
    Update-CheckBuildLock $held
    & "$checkout\rust\tests\build-pyrowave-client.ps1" -ArtifactDirectory $fixtures -PyrowaveRoot $env:BUTTERPOLLO_PYROWAVE_ROOT *> "$qa\pyrowave-receiver-build.log"
    Update-CheckBuildLock $held
    cargo build --release --locked --manifest-path "$checkout\Cargo.toml" --target-dir "$checkout\target\ship" -p butterpollo-windows `
        --example audio_probe --example motion_probe --example udp_shaper *> "$qa\probe-build.log"
} finally { Exit-CheckLocks $held }
# udp_shaper is for bottleneck.py, the slow-link check, which this check does not run.
Copy-Item "$checkout\target\ship\release\examples\audio_probe.exe", "$checkout\target\ship\release\examples\motion_probe.exe",
    "$checkout\target\ship\release\examples\udp_shaper.exe" $fixtures
$client = "$fixtures\moonlight-client.exe"

foreach ($case in 'h264', 'hevc', 'av1', 'hevc-vrr', 'hevc-recovery', 'hevc-120', 'hevc-warp', 'pyrowave', 'pyrowave-hdr-444') {
    if ($SkipStreams -contains $case) { Write-Warning "stream $case skipped (-SkipStreams); recorded in VALIDATION.json"; continue }
    $codec = $case -replace '-(vrr|recovery|120|warp)$'
    # 12 s of streaming each: enough for the steady-rate, motion and audio checks.
    $stream = @('--package', $package, '--work', $run, '--client', $client, '--codec', $codec, '--seconds', '12')
    if ($case.EndsWith('-vrr')) { $stream += '--vrr' }
    # Above 60 FPS at a higher bitrate; recorded as skipped below a 120 Hz display.
    # The receiver decodes on the GPU: software HEVC managed about 65 FPS here.
    if ($case.EndsWith('-120')) {
        $stream += '--mode', '2560x1440x120', '--bitrate', '80000', '--name', $case, '--hw-decoder', 'd3d11va'
    }
    # Artemis's Warp frame pacing: launched at 60, asking for 120. The stream
    # runs at the request's rate (60 before 2.3.1).
    if ($case.EndsWith('-warp')) {
        $stream += '--mode', '2560x1440x120', '--bitrate', '80000', '--name', $case, '--hw-decoder', 'd3d11va', '--launch-fps', '60'
    }
    # A client losing packets: 16 keyframe requests, one every 0.5 s, must
    # not send pictures twice or skip new frames (rc.27-rc.28 did).
    if ($case.EndsWith('-recovery')) { $stream += '--recovery', '16' }
    if ($codec.StartsWith('pyrowave')) {
        $stream = @('--package', $package, '--work', $run, '--client', "$fixtures\moonlight-pyrowave-client.exe",
                    '--codec', $codec, '--mode', '1920x1080x60', '--seconds', '12', '--bitrate', '400000')
    }
    Step "stream $case"
    Invoke-StreamCase $case $stream $run
}
Step 'protocol checks'
& $python "$tools\protocol.py" --package $package --work $run

if (-not $NoInstall) {
    Remove-Item -Recurse -Force "$run\elevated" -ErrorAction SilentlyContinue
    # elevation.ps1 installs a task that runs this step without a prompt. It
    # is used while its scripts match this checkout's and -Work is the default.
    $taskHome = 'C:\ProgramData\ButterpolloRelease'
    $task = $false
    try { schtasks /Query /TN ButterpolloReleaseElevated *> $null; $task = $true } catch { }
    $current = $task -and $Work -eq 'C:\src\butterpollo-release' -and
        @('elevated.ps1', 'elevated-task.ps1' | Where-Object {
            -not (Test-Path "$taskHome\$_") -or (Get-FileHash "$taskHome\$_").Hash -ne (Get-FileHash "$tools\$_").Hash
        }).Count -eq 0
    $admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator)
    $arguments = '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$tools\elevated.ps1`"",
        '-Package', "`"$package`"", '-Installer', "`"$installer`"", '-Version', $Version, '-Work', "`"$run`""
    if ($admin) {
        Step 'display self-test as SYSTEM and install'
        # Started from here, Windows PowerShell inherits PowerShell 7's module
        # path and cannot load Get-FileHash; give it its own.
        $modulePath = $env:PSModulePath
        $env:PSModulePath = [Environment]::GetEnvironmentVariable('PSModulePath', 'Machine')
        try { Start-Process powershell -WindowStyle Hidden -ArgumentList $arguments }
        finally { $env:PSModulePath = $modulePath }
    } elseif ($current) {
        Step 'display self-test as SYSTEM and install'
        @{ version = $Version } | ConvertTo-Json | Set-Content (Join-Path $Work 'request.json')
        schtasks /Run /TN ButterpolloReleaseElevated | Out-Null
    } else {
        if ($task) { Write-Warning 'The release elevation task is out of date; run rust/release/elevation.ps1 as administrator again.' }
        Step 'display self-test as SYSTEM and install (UAC prompt)'
        Start-Process powershell -Verb RunAs -WindowStyle Hidden -ArgumentList $arguments
    }
    $deadline = (Get-Date).AddMinutes(15)
    while (-not (Test-Path "$run\elevated\done.txt")) {
        if ((Get-Date) -gt $deadline) { throw 'The elevated step did not finish; was the UAC prompt declined?' }
        Start-Sleep -Seconds 2
    }
    if (-not (Test-Path "$run\elevated\installed.json")) { throw "The install did not finish; see $run\elevated\transcript.txt" }
    if (-not (Test-Path "$run\elevated\display-self-test.json")) {
        Write-Warning "The display self-test wrote no report; see $run\elevated\transcript.txt"
    }
}

Step 'record results'
$gpu = (Get-CimInstance Win32_VideoController | Where-Object Name -NotMatch 'Virtual|Basic|Idd' | Select-Object -First 1).Name
$record = @('--version', $Version, '--package', $package, '--work', $run, '--out', "$run\VALIDATION.json",
            '--scope', "Release workstation: $gpu, Windows $([Environment]::OSVersion.Version.Build). Other hardware was not tested.")
$record += @('--seconds', [int]((Get-Date) - $started).TotalSeconds)
if ($RealClient) { $record += @('--real-client', $RealClient) }
if (-not $NoInstall) { $record += '--installed' }
foreach ($skip in $SkipStreams) { $record += @('--skipped', $skip) }
& $python "$tools\validation.py" @record
if ($NoUpload) {
    Step "done, not uploaded: $run\VALIDATION.json"
    return
}
gh release upload $tag -R $repo --clobber "$run\VALIDATION.json"
# gh writes UTF-8; read in the console's code page, every non-ASCII
# character in the release text came back garbled (rc.23).
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$body = gh release view $tag -R $repo --json body --jq .body
if ($body -notmatch 'VALIDATION\.json') {
    $body = ($body -join "`n").TrimEnd() +
        " · [Host validation](https://github.com/$repo/releases/download/$tag/VALIDATION.json)`n"
    [IO.File]::WriteAllText("$run\body.md", $body, [Text.UTF8Encoding]::new($false))
    gh release edit $tag -R $repo --notes-file "$run\body.md"
}
Step "checked https://github.com/$repo/releases/tag/$tag"
