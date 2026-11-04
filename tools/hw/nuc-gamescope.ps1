# nightjar-gamescope-nuc: Nightjar desktop under headless gamescope 1280x800@60 on the NUC, the
# stand-in for a Steam Deck in Game Mode. Syncs this checkout's client/protocol to the NUC, builds
# it in a throwaway Arch container (tools/hw/nuc-gamescope/container.sh: gamescope is not packaged
# for Debian), starts the isolated test host on this PC (never the installed one) and runs
# nuc-gamescope/batch.sh: 60 s streams outside gamescope, under it, under it with HDR, the
# launcher driven by a virtual Xbox pad alone, and the release's Flatpak under gamescope with and
# without its gamescope socket. The Flatpak bundle comes from nuc-gamescope/release-linux.sh
# (release.yml's Linux steps: package-linux.sh, build-bundle.sh, check-bundle.sh) with
# -BuildBundle, else the last one it built. Holds GPU-LOCK while the host runs (check_streams.ps1's
# lock helpers: taken atomically, a lock of the run.py batch that started this counts as ours, and
# only the text this run wrote is removed). rust/release/gpu_hang.py watches the host log live and
# stops the batch at the first sign of an AMD encoder hang. The container, the Flatpak's own HOME
# and the host process go again at the end unless -Keep. -SelfTest checks the lock and port
# handling against a temporary folder, without the NUC or a host.
# Exit code: the batch's (0 when every check passes), 3 for an AMD encoder hang: stop, never retry.
param(
    [string] $Out,
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
    [string] $LockDirectory = 'C:\src\butterpollo-release',
    [switch] $BuildBundle,
    [switch] $Keep,
    [switch] $SelfTest
)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path "$PSScriptRoot\..\..").Path
. "$repo\rust\release\check_streams.ps1"
function nuc([string] $command) {
    ssh nuc $command
    if ($LASTEXITCODE -ne 0) { throw "ssh nuc failed ($LASTEXITCODE): $command" }
}

# GPU-LOCK for this run: '' when the batch that started this holds it (run.py writes "pid N"),
# else the text written (CreateNew, so two sessions never both take it). Throws when another
# session holds it, also when its process is gone: GPU-LOCK is never broken.
function Enter-GpuLock {
    if (Test-LockOurs $script:CheckGpuLock) {
        Write-Host "GPU-LOCK held by the batch that started this: $(Read-Lock $script:CheckGpuLock)"
        return ''
    }
    $written = Lock-Try $script:CheckGpuLock $Tag
    if ($written) { return $written }
    $held = Read-Lock $script:CheckGpuLock
    $gone = if ($held -match '\bpid[= ](\d+)' -and -not (Get-Process -Id $Matches[1] -ErrorAction SilentlyContinue)) {
        " (pid $($Matches[1]) is not running: remove $script:CheckGpuLock by hand once that session is surely gone)"
    }
    throw "GPU-LOCK is held: $held$gone"
}

# Nothing may listen on the host's HTTP port before it starts: the readiness poll would take
# another server's answer for the isolated host's.
function Assert-PortFree([int] $Port) {
    $owners = @(Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue | Select-Object -ExpandProperty OwningProcess -Unique)
    if ($owners) {
        $names = ($owners | ForEach-Object { "pid $_ ($((Get-Process -Id $_ -ErrorAction SilentlyContinue).ProcessName))" }) -join ', '
        throw "port $Port is already in use by ${names}: not starting the isolated host beside it"
    }
}

if ($SelfTest) {
    $dir = Join-Path ([IO.Path]::GetTempPath()) "nuc-gamescope-selftest-$PID"
    New-Item -ItemType Directory -Force $dir | Out-Null
    $failures = [Collections.Generic.List[string]]::new()
    function expect([string] $what, [bool] $ok) { if ($ok) { Write-Host "ok   $what" } else { Write-Host "FAIL $what"; $failures.Add($what) } }
    try {
        Initialize-CheckLocks $dir 3>$null
        $lock = $script:CheckGpuLock
        $parent = (Get-CimInstance Win32_Process -Filter "ProcessId = $PID").ParentProcessId
        # Another session's run.py lock, its process alive and not one that started this: the
        # System process, pid 4.
        Set-Content $lock 'tools/hw/run.py --batch lab pid 4 since 2026-11-04 10:00' -NoNewline
        $refused = try { Enter-GpuLock | Out-Null; $false } catch { $_ -match 'GPU-LOCK is held: tools/hw/run.py' }
        expect "another session's 'pid N' lock is refused" $refused
        expect 'the refused lock is left as it was' ((Read-Lock $lock) -ceq 'tools/hw/run.py --batch lab pid 4 since 2026-11-04 10:00')
        Set-Content $lock "nuc-gamescope 2026-11-04T10:00:00 pid=999999" -NoNewline
        $refused = try { Enter-GpuLock | Out-Null; $false } catch { $_ -match 'is not running: remove' }
        expect 'a lock of an exited process is refused, not broken' $refused
        Set-Content $lock "tools/hw/run.py --batch nuc pid $parent since 2026-11-04 10:00" -NoNewline
        expect "the parent batch's lock counts as ours" ((Enter-GpuLock) -eq '')
        Lock-Release $lock ''
        expect "the parent batch's lock stays after the run" ([IO.File]::Exists($lock))
        Remove-Item $lock
        $written = Enter-GpuLock
        expect 'a free lock is taken with this run''s pid' ($written -match "^$Tag .* pid=$PID$" -and (Read-Lock $lock) -ceq $written)
        Lock-Release $lock $written
        expect 'this run''s lock is removed at the end' (-not [IO.File]::Exists($lock))
        $written = Enter-GpuLock
        Set-Content $lock 'check.ps1 2026-11-04T10:00:00 pid=4' -NoNewline
        Lock-Release $lock $written 6>$null
        expect 'a lock taken over by another session stays' ((Read-Lock $lock) -ceq 'check.ps1 2026-11-04T10:00:00 pid=4')
        $listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
        $listener.Start()
        try {
            $busy = $listener.LocalEndpoint.Port
            $named = try { Assert-PortFree $busy; $false } catch { $_ -match "port $busy is already in use by pid $PID " }
            expect 'a port in use is refused, naming its owner' $named
        } finally { $listener.Stop() }
        $free = try { Assert-PortFree $busy; $true } catch { $false }
        expect 'a free port passes' $free
    } finally { Remove-Item -Recurse -Force $dir -ErrorAction SilentlyContinue }
    if ($failures.Count) { Write-Host "$($failures.Count) self-test check(s) failed"; exit 1 }
    Write-Host 'self-test passed'
    exit 0
}
if (-not $Out) { throw '-Out is required' }
New-Item -ItemType Directory -Force $Out | Out-Null
$python = (Get-Command python -ErrorAction Stop).Source

$info = (Invoke-WebRequest -UseBasicParsing http://127.0.0.1:47989/serverinfo -TimeoutSec 5).Content
if ($info -notmatch '<RustHostSessionCount>0<' -or $info -notmatch '<RustHostPendingSessionCount>0<') {
    throw 'the installed host has a session: not now'
}
Initialize-CheckLocks $LockDirectory
# Early, before the build: the lock is taken for the streams only.
if ([IO.File]::Exists($script:CheckGpuLock) -and -not (Test-LockOurs $script:CheckGpuLock)) { throw "GPU-LOCK is held: $(Read-Lock $script:CheckGpuLock)" }

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
$watch = $null
$batch = $null
$lockWritten = $null
$code = 1
$evidence = "$Out\gpu-hang.txt"
$watchStop = "$Out\gpu-hang-watch.stop"
$watchReady = "$Out\gpu-hang-watch.ready"
try {
    $lockWritten = Enter-GpuLock
    Assert-PortFree $Port
    Remove-Item $evidence, $watchStop, $watchReady -ErrorAction SilentlyContinue
    $before = if (Test-Path $hostLog) { (Get-Item $hostLog).Length } else { 0 }
    # The hang watch takes the log's size before the host starts, so it reads every line this
    # run's host writes, and polls it (and the display driver's watchdog reports) every 0.5 s.
    $watch = Start-Process $python -ArgumentList "`"$repo\rust\release\gpu_hang.py`"", "`"$hostLog`"", "`"$evidence`"", "`"$watchStop`"", "`"$watchReady`"" `
        -NoNewWindow -PassThru -RedirectStandardOutput "$Out\gpu-hang-watch.stdout.log" -RedirectStandardError "$Out\gpu-hang-watch.stderr.log"
    $null = $watch.Handle
    $deadline = (Get-Date).AddSeconds(15)
    while (-not (Test-Path $watchReady)) {
        if ($watch.HasExited -or (Get-Date) -gt $deadline) { throw "the hang watch did not start: see $Out\gpu-hang-watch.stderr.log" }
        Start-Sleep -Milliseconds 100
    }
    Write-Host '== isolated host'
    $hostProcess = Start-Process $HostExe -ArgumentList '--config-dir', $HostConfig, '--port', $Port, '--bind', $Bind `
        -WindowStyle Hidden -PassThru -RedirectStandardOutput "$Out\host.stdout.log" -RedirectStandardError "$Out\host.stderr.log"
    $null = $hostProcess.Handle
    $deadline = (Get-Date).AddSeconds(30)
    while ($true) {
        try { Invoke-WebRequest -UseBasicParsing "http://${Bind}:$Port/serverinfo" -TimeoutSec 2 | Out-Null; break } catch {}
        if ($hostProcess.HasExited -or (Get-Date) -gt $deadline) { throw 'the isolated host did not start' }
        Start-Sleep -Milliseconds 500
    }
    if ($hostProcess.HasExited) { throw "the isolated host exited (code $($hostProcess.ExitCode)); something else answered on port $Port" }
    $listeners = @(Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue | Select-Object -ExpandProperty OwningProcess -Unique)
    if ($listeners -notcontains $hostProcess.Id) { throw "port $Port answers from pid $($listeners -join ', '), not the isolated host (pid $($hostProcess.Id))" }
    Write-Host '== batch'
    # ssh joins its arguments into the remote command line.
    $batch = Start-Process ssh -ArgumentList 'nuc', "W=$Work bash $Work/tools/batch.sh $bundle" `
        -NoNewWindow -PassThru -RedirectStandardOutput "$Out\batch.txt" -RedirectStandardError "$Out\batch.stderr.txt"
    $null = $batch.Handle
    while (-not $batch.HasExited -and -not $watch.HasExited) { Start-Sleep -Milliseconds 500 }
    if ($batch.HasExited) {
        # Stops the watch once it has read the log to the end (and waited out a stall's grace).
        Set-Content $watchStop ''
        if (-not $watch.WaitForExit(15000)) { throw 'the hang watch did not stop' }
    }
    if ($watch.ExitCode -eq $EXIT_GPU_HANG) {
        Write-Host "AMD encoder hang: $(Get-Content $evidence -Raw)" -ForegroundColor Red
        Write-Host 'Stopping the batch. Do not retry: reboot if Device Manager shows the Radeon with Code 31.' -ForegroundColor Red
        if (-not $batch.HasExited) {
            Stop-Process -Id $batch.Id -Force -ErrorAction SilentlyContinue
            ssh nuc "pkill -f 'tools/batch.sh'; docker exec nj-gamescope pkill -f 'run.sh|nightjar|gamescope'"
        }
        $code = $EXIT_GPU_HANG
    } elseif ($watch.ExitCode -ne 0) {
        throw "the hang watch failed (exit $($watch.ExitCode)): see $Out\gpu-hang-watch.stderr.log"
    } else {
        $code = $batch.ExitCode
    }
    Get-Content "$Out\batch.txt"
    scp -q -r "nuc:$Work/runs" "$Out\"
    if (Test-Path $hostLog) {
        $stream = [IO.File]::Open($hostLog, 'Open', 'Read', 'ReadWrite')
        $stream.Seek($before, 'Begin') | Out-Null
        $text = (New-Object IO.StreamReader $stream).ReadToEnd()
        $stream.Dispose()
        Set-Content "$Out\host.log" $text
        ($text -split "`n") | Where-Object { $_ -match 'stream timings' } | Set-Content "$Out\host-stream-timings.txt"
    }
} finally {
    if ($batch -and -not $batch.HasExited) { Stop-Process -Id $batch.Id -Force -ErrorAction SilentlyContinue }
    if ($hostProcess -and -not $hostProcess.HasExited) { Stop-Process -Id $hostProcess.Id -Force }
    if ($watch -and -not $watch.HasExited) { Stop-Process -Id $watch.Id -Force -ErrorAction SilentlyContinue }
    if ($lockWritten) { Lock-Release $script:CheckGpuLock $lockWritten }
    if (-not $Keep) {
        ssh nuc "docker exec nj-gamescope rm -rf $Work/fphome; W=$Work bash $Work/tools/container.sh rm"
    }
}
exit $code
