# nightjar-gamescope-nuc: Nightjar desktop under headless gamescope 1280x800@60 on the NUC, the
# stand-in for a Steam Deck in Game Mode. Syncs this checkout's client/protocol to the NUC, builds
# it in a throwaway Arch container (tools/hw/nuc-gamescope/container.sh: gamescope is not packaged
# for Debian), starts the isolated test host on this PC (never the installed one) and runs
# nuc-gamescope/batch.sh: 60 s streams outside gamescope, under it, under it with HDR, the
# launcher driven by a virtual Xbox pad alone, and the release's Flatpak under gamescope with and
# without its gamescope socket. The Flatpak bundle comes from nuc-gamescope/release-linux.sh
# (release.yml's Linux steps: package-linux.sh, build-bundle.sh, check-bundle.sh) with
# -BuildBundle, else the last one it built. Holds GPU-LOCK while the host runs. The container,
# the Flatpak's own HOME and the host process go again at the end unless -Keep.
# Exit code: the batch's (0 when every check passes), 3 for an AMD encoder hang in the host log.
param(
    [Parameter(Mandatory)] [string] $Out,
    # The isolated host and its profile: Desktop only, virtual display and display changes off,
    # the Steam Streaming Speakers as a capture-only sink (device-stream-isolated-host).
    [string] $HostExe = 'C:\src\butterpollo-release\nj-e2e-host\butterpollo.exe',
    [string] $HostConfig = 'C:\tests\njrf',
    [string] $Bind = '192.168.4.69',
    [int] $Port = 48463,
    # The work folder on the NUC.
    [string] $Work = '~/deck',
    [string] $Version = '4.5.0',
    [string] $Tag = 'nuc-gamescope',
    [switch] $BuildBundle,
    [switch] $Keep
)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path "$PSScriptRoot\..\..").Path
New-Item -ItemType Directory -Force $Out | Out-Null
function nuc([string] $command) {
    ssh nuc $command
    if ($LASTEXITCODE -ne 0) { throw "ssh nuc failed ($LASTEXITCODE): $command" }
}

$info = (Invoke-WebRequest -UseBasicParsing http://127.0.0.1:47989/serverinfo -TimeoutSec 5).Content
if ($info -notmatch '<RustHostSessionCount>0<' -or $info -notmatch '<RustHostPendingSessionCount>0<') {
    throw 'the installed host has a session: not now'
}
$lock = 'C:\src\butterpollo-release\GPU-LOCK'
if (Test-Path $lock) {
    $held = Get-Content $lock -Raw
    if ($held -match 'pid=(\d+)' -and (Get-Process -Id $Matches[1] -ErrorAction SilentlyContinue)) { throw "GPU-LOCK is held: $held" }
}

Write-Host '== sync and build on the NUC'
nuc "mkdir -p $Work/src $Work/tools"
cmd /c "tar -czf - -C `"$repo`" --exclude=client/protocol/vendor/sdl3-src/SDL --exclude=target client/protocol client/app/build.gradle rust-toolchain.toml | ssh nuc `"tar -xzf - -C $Work/src`""
if ($LASTEXITCODE -ne 0) { throw 'sync failed' }
# tar keeps the checkout's times: build.rs reads the version from build.gradle only when it looks newer.
nuc "touch $Work/src/client/app/build.gradle"
cmd /c "tar -czf - -C `"$repo\tools\hw\nuc-gamescope`" . | ssh nuc `"tar -xzf - -C $Work/tools`""
if ($LASTEXITCODE -ne 0) { throw 'copying the scripts failed' }
nuc "test -d $Work/src/client/protocol/vendor/sdl3-src/SDL/include || { echo 'no SDL source in $Work/src (git submodule update --init client/protocol/vendor/sdl3-src/SDL here, then copy it)'; exit 1; }"
ssh nuc "docker inspect -f '{{.State.Running}}' nj-gamescope 2>/dev/null | grep -q true || W=$Work bash $Work/tools/container.sh setup"
if ($LASTEXITCODE -ne 0) { throw 'container setup failed' }
nuc "W=$Work bash $Work/tools/container.sh build"
if ($BuildBundle) {
    ssh nuc "W=$Work V=$Version bash $Work/tools/release-linux.sh all" | Tee-Object "$Out\release-linux.txt"
    if (Select-String -Path "$Out\release-linux.txt" -Pattern 'FAILED|exit [1-9]' -Quiet) { throw 'release-linux.sh failed: see release-linux.txt' }
}
$bundle = "$Work/rel/dist/nightjar-desktop-$Version-x86_64.flatpak"
nuc "test -s $bundle || { echo 'no ${bundle}, run with -BuildBundle'; exit 1; }"

$hostLog = Join-Path $HostConfig 'logs\butterpollo.log'
$hostProcess = $null
$code = 1
try {
    Set-Content $lock "$Tag $(Get-Date -Format s) pid=$PID" -NoNewline
    $before = if (Test-Path $hostLog) { (Get-Item $hostLog).Length } else { 0 }
    Write-Host '== isolated host'
    $hostProcess = Start-Process $HostExe -ArgumentList '--config-dir', $HostConfig, '--port', $Port, '--bind', $Bind `
        -WindowStyle Hidden -PassThru -RedirectStandardOutput "$Out\host.stdout.log" -RedirectStandardError "$Out\host.stderr.log"
    $deadline = (Get-Date).AddSeconds(30)
    while ($true) {
        try { Invoke-WebRequest -UseBasicParsing "http://${Bind}:$Port/serverinfo" -TimeoutSec 2 | Out-Null; break } catch {}
        if ($hostProcess.HasExited -or (Get-Date) -gt $deadline) { throw 'the isolated host did not start' }
        Start-Sleep -Milliseconds 500
    }
    Write-Host '== batch'
    ssh nuc "W=$Work bash $Work/tools/batch.sh $bundle" | Tee-Object "$Out\batch.txt"
    $code = $LASTEXITCODE
    scp -q -r "nuc:$Work/runs" "$Out\"
    if (Test-Path $hostLog) {
        $stream = [IO.File]::Open($hostLog, 'Open', 'Read', 'ReadWrite')
        $stream.Seek($before, 'Begin') | Out-Null
        $text = (New-Object IO.StreamReader $stream).ReadToEnd()
        $stream.Dispose()
        Set-Content "$Out\host.log" $text
        ($text -split "`n") | Where-Object { $_ -match 'stream timings' } | Set-Content "$Out\host-stream-timings.txt"
        if ($text -match '(?i)encoder hang|AMF.*hang') { Write-Host 'AMD encoder hang in the host log: stop'; $code = 3 }
    }
} finally {
    if ($hostProcess -and -not $hostProcess.HasExited) { Stop-Process -Id $hostProcess.Id -Force }
    Remove-Item $lock -ErrorAction SilentlyContinue
    if (-not $Keep) {
        ssh nuc "docker exec nj-gamescope rm -rf $Work/fphome; W=$Work bash $Work/tools/container.sh rm"
    }
}
exit $code
