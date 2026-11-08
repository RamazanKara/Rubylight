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
# else the text written (CreateNew, so two sessions never both take it). Another session's lock
# is waited for, polled every 3 s like run.py and takelocks.sh do, for up to $Wait seconds; then
# it throws, also when the holder's process is gone: GPU-LOCK is never broken.
function Enter-GpuLock([double] $Wait = 1800) {
    if (Test-LockOurs $script:CheckGpuLock) {
        Write-Host "GPU-LOCK held by the batch that started this: $(Read-Lock $script:CheckGpuLock)"
        return ''
    }
    $deadline = (Get-Date).AddSeconds($Wait)
    $shown = $null
    while ($true) {
        $written = Lock-Try $script:CheckGpuLock $Tag
        if ($written) { return $written }
        $held = Read-Lock $script:CheckGpuLock
        if ((Get-Date) -ge $deadline) { break }
        if ($held -and $held -ne $shown) { Write-Host "waiting for GPU-LOCK: $held"; $shown = $held }
        Start-Sleep -Seconds 3
    }
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

# Stops a batch still running on the NUC: batch.sh, then the runs in the container. batch.sh has
# no terminal, so a closed ssh does not stop it. The bracket keeps pkill -f from matching the
# remote shell that runs it, whose command line holds the pattern.
$RemoteStop = @("pkill -f '[t]ools/batch.sh'", "docker exec nj-gamescope pkill -f '[r]un.sh|[n]ightjar|[g]amescope'")

# The sync's extract on the NUC. -m gives each file the time it arrives: with the checkout's own
# times, a changed file older than the last build looks built already and cargo links the old crate.
$SyncExtract = 'tar -xzmf -'
function Stop-RemoteBatch {
    foreach ($command in $RemoteStop) {
        ssh -n nuc $command
        # pkill: 0 stopped some, 1 found none; ssh: 255 did not reach the NUC.
        if ($LASTEXITCODE -gt 1) { Write-Host "stopping the batch on the NUC failed ($LASTEXITCODE): $command" -ForegroundColor Red }
    }
}

# Runs every step; a step that throws is said and makes the code at least 1, but exit 3 (a hang)
# stays 3. What a step prints goes to the console, not into the code returned.
function Invoke-Steps([int] $Code, [scriptblock[]] $Steps) {
    foreach ($step in $Steps) {
        try { & $step | Out-Host } catch {
            Write-Host "failed: $_" -ForegroundColor Red
            if ($Code -ne $EXIT_GPU_HANG) { $Code = [math]::Max($Code, 1) }
        }
    }
    $Code
}

# After the batch: $true once the host log has not grown for $Quiet seconds, $false if it still
# grows after $Max. The serverinfo session counts are empty to an unpaired caller, so the log is
# the sign that the last stream's teardown is done. A hung encoder call writes nothing until it
# returns, but the stall watch's line comes at 0.5 s, before the quiet ends; gpu_hang.watch then
# waits out its grace.
function Wait-LogQuiet([string] $Path, [double] $Quiet = 3, [double] $Max = 30, [scriptblock] $Until = { $false }) {
    $start = Get-Date
    $size = -1
    $since = $start
    while (-not (& $Until)) {
        $now = Get-Date
        $current = if ([IO.File]::Exists($Path)) { ([IO.FileInfo]::new($Path)).Length } else { 0 }
        if ($current -ne $size) { $size = $current; $since = $now }
        if (($now - $since).TotalSeconds -ge $Quiet) { return $true }
        if (($now - $start).TotalSeconds -ge $Max) { return $false }
        Start-Sleep -Milliseconds 250
    }
    $true
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
        $refused = try { Enter-GpuLock 0 | Out-Null; $false } catch { $_ -match 'GPU-LOCK is held: tools/hw/run.py' }
        expect "another session's 'pid N' lock is refused" $refused
        expect 'the refused lock is left as it was' ((Read-Lock $lock) -ceq 'tools/hw/run.py --batch lab pid 4 since 2026-11-04 10:00')
        Set-Content $lock "nuc-gamescope 2026-11-04T10:00:00 pid=999999" -NoNewline
        $refused = try { Enter-GpuLock 0 | Out-Null; $false } catch { $_ -match 'is not running: remove' }
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
        # Another session's lock, released while this run waits: taken then.
        Set-Content $lock 'njrf 2026-11-04T05:40:11 pid=4' -NoNewline
        $release = Start-Job { param($p) Start-Sleep -Seconds 2; Remove-Item $p } -ArgumentList $lock
        try {
            $written = Enter-GpuLock 30 6>$null
            expect 'a lock released while waiting is taken then' ($written -and (Read-Lock $lock) -ceq $written)
            Lock-Release $lock $written
        } finally { $release | Wait-Job | Remove-Job }
        $t = Get-Date
        Set-Content $lock 'njrf 2026-11-04T05:40:11 pid=4' -NoNewline
        $refused = try { Enter-GpuLock 1 6>$null | Out-Null; $false } catch { $_ -match 'GPU-LOCK is held: njrf' }
        expect 'a lock still held after the wait is refused' ($refused -and ((Get-Date) - $t).TotalSeconds -lt 10)
        Remove-Item $lock
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
        # The remote stop: each pkill pattern (an ERE, read here as a .NET regex) finds the batch's
        # processes but not the remote shell running the pkill.
        $patterns = $RemoteStop | ForEach-Object { if ($_ -match "pkill -f '([^']+)'") { $Matches[1] } }
        expect 'the remote stop names batch.sh and the runs' ($patterns.Count -eq 2 -and
            'bash /home/rambo/deck/tools/batch.sh /home/rambo/deck/rel/dist/nightjar-desktop-4.5.0-x86_64.flatpak' -match $patterns[0] -and
            'bash /home/rambo/deck/tools/run.sh s-gs gs 0 auto' -match $patterns[1] -and '/usr/bin/nightjar --autostream' -match $patterns[1])
        # The second pkill runs in the container, which does not see the NUC's shell.
        expect "the remote stop's own shell is not matched" ("bash -c $($RemoteStop[0])" -notmatch $patterns[0] -and "bash -c $($RemoteStop[1])" -notmatch $patterns[0])
        expect "a plain pattern would match its own shell (the old bug)" ("pkill -f 'tools/batch.sh'" -match 'tools/batch.sh')
        # Collecting results after a hang: a failing step keeps exit 3, else makes it 1.
        $ran = [Collections.Generic.List[int]]::new()
        $kept = Invoke-Steps 3 @({ throw 'no host log' }, { $ran.Add(2) }) 6>$null
        expect 'a failing step after a hang keeps exit 3 and the next step runs' ($kept -eq 3 -and $ran.Contains(2))
        expect 'a failing step otherwise gives exit 1' ((Invoke-Steps 0 @({ throw 'scp failed' }) 6>$null) -eq 1)
        expect 'steps that pass keep the code' ((Invoke-Steps 0 @({ }, { })) -eq 0 -and (Invoke-Steps 2 @({ })) -eq 2)
        $printed = Invoke-Steps 0 @({ 'batch.txt line 1'; 'line 2' }) 6>$null
        expect 'a step that prints still returns the code alone' ($printed -is [int] -and $printed -eq 0)
        # The bundle's FFmpeg: built again whenever the files release.yml keys its FFmpeg cache by
        # change (a prefix from before --enable-libdrm failed every VAAPI map with -38).
        $releaseLinux = Get-Content -Raw "$PSScriptRoot\nuc-gamescope\release-linux.sh"
        $key = (Select-String -Path "$repo\client\.github\workflows\release.yml" -Pattern 'key: ffmpeg-decode-only-linux').Line
        $keyed = [regex]::Matches($key, "'(protocol/desktop/[^']+)'") | ForEach-Object { $_.Groups[1].Value }
        $recipe = ($releaseLinux -split "`n" | Where-Object { $_ -match '^\s*cat .*sha256sum' }) -join ''
        expect 'the FFmpeg recipe hashes what release.yml keys its FFmpeg cache by' ($keyed.Count -ge 3 -and
            @($keyed | Where-Object { $recipe -notmatch [regex]::Escape("`"`$1`"/$_") }).Count -eq 0)
        expect 'a prefix of another recipe is built again' ($releaseLinux -match [regex]::Escape('[ "$(cat $rel/ffprefix/recipe.sha256 2>/dev/null)" != "$recipe" ]') -and
            $releaseLinux -match [regex]::Escape('echo $recipe >$rel/ffprefix/recipe.sha256'))
        # The end of the batch: the watch reads on while the host log grows.
        $log = Join-Path $dir 'butterpollo.log'
        Set-Content $log 'start'
        $t = Get-Date
        expect 'a quiet log ends the wait after the quiet time' ((Wait-LogQuiet $log 0.5 5) -and ((Get-Date) - $t).TotalSeconds -lt 3)
        $writer = Start-Job { param($p) for ($i = 0; $i -lt 40; $i++) { Add-Content $p "line $i"; Start-Sleep -Milliseconds 100 } } -ArgumentList $log
        try {
            Start-Sleep -Milliseconds 1500
            expect 'a growing log is not quiet' (-not (Wait-LogQuiet $log 1 1.5))
        } finally { $writer | Stop-Job -PassThru | Remove-Job }
        expect 'the watch ending ends the wait' (Wait-LogQuiet $log 10 10 -Until { $true })
        # The sync: a file older than the NUC's last build still counts as changed there.
        $old = Join-Path $dir 'old.txt'
        Set-Content $old 'x'
        (Get-Item $old).LastWriteTime = [datetime]'2020-01-01'
        $synced = New-Item -ItemType Directory (Join-Path $dir 'synced')
        cmd /c "tar -czf - -C `"$dir`" old.txt | $SyncExtract -C `"$synced`""
        expect "the sync gives a file the time it arrives" ((Get-Item (Join-Path $synced 'old.txt')).LastWriteTime -gt (Get-Date).AddMinutes(-5))
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
# The lock is taken for the streams only, after the build; said early when another session has it.
if ([IO.File]::Exists($script:CheckGpuLock) -and -not (Test-LockOurs $script:CheckGpuLock)) { Write-Host "GPU-LOCK is held now, waited for before the streams: $(Read-Lock $script:CheckGpuLock)" }

Write-Host '== sync and build on the NUC'
nuc "mkdir -p $Work/src $Work/tools"
cmd /c "tar -czf - -C `"$repo`" --exclude=client/protocol/vendor/sdl3-src/SDL --exclude=target client/protocol client/app/build.gradle rust-toolchain.toml | ssh nuc `"$SyncExtract -C $Work/src`""
if ($LASTEXITCODE -ne 0) { throw 'sync failed' }
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
$idleLate = $false
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
    # ssh joins its arguments into the remote command line; -n: no console stdin.
    $batch = Start-Process ssh -ArgumentList '-n', 'nuc', "W=$Work bash $Work/tools/batch.sh $bundle" `
        -NoNewWindow -PassThru -RedirectStandardOutput "$Out\batch.txt" -RedirectStandardError "$Out\batch.stderr.txt"
    $null = $batch.Handle
    while (-not $batch.HasExited -and -not $watch.HasExited) { Start-Sleep -Milliseconds 500 }
    if ($batch.HasExited) {
        # The host still logs the last stream's teardown (6 s on a reset GPU): the watch reads on
        # until the log is quiet, then the host stops, then the watch reads to the end.
        if (-not (Wait-LogQuiet $hostLog -Until { $watch.HasExited })) {
            Write-Host "the isolated host's log still grows 30 s after the batch's end" -ForegroundColor Red
            $idleLate = $true
        }
        Stop-Process -Id $hostProcess.Id -Force -ErrorAction SilentlyContinue
        Set-Content $watchStop ''
        if (-not $watch.WaitForExit(15000)) { throw 'the hang watch did not stop' }
    }
    if ($watch.ExitCode -eq $EXIT_GPU_HANG) {
        $code = $EXIT_GPU_HANG
        Stop-Process -Id $hostProcess.Id -Force -ErrorAction SilentlyContinue
        if (-not $batch.HasExited) { Stop-Process -Id $batch.Id -Force -ErrorAction SilentlyContinue }
        Stop-RemoteBatch
        Write-Host "AMD encoder hang: $(Get-Content $evidence -Raw -ErrorAction SilentlyContinue)" -ForegroundColor Red
        Write-Host 'Stopped the batch and the host. Do not retry: reboot if Device Manager shows the Radeon with Code 31.' -ForegroundColor Red
    } elseif ($watch.ExitCode -ne 0) {
        throw "the hang watch failed (exit $($watch.ExitCode)): see $Out\gpu-hang-watch.stderr.log"
    } else {
        $code = if ($idleLate) { [math]::Max($batch.ExitCode, 1) } else { $batch.ExitCode }
    }
    # Collecting the results never turns exit 3 into another code.
    $code = Invoke-Steps $code @(
        { Get-Content "$Out\batch.txt" }
        { scp -q -r "nuc:$Work/runs" "$Out\"; if ($LASTEXITCODE -ne 0) { throw "scp failed ($LASTEXITCODE)" } }
        {
            if (Test-Path $hostLog) {
                $stream = [IO.File]::Open($hostLog, 'Open', 'Read', 'ReadWrite')
                try {
                    if ($stream.Length -ge $before) { $stream.Seek($before, 'Begin') | Out-Null }
                    $text = (New-Object IO.StreamReader $stream).ReadToEnd()
                } finally { $stream.Dispose() }
                Set-Content "$Out\host.log" $text
                ($text -split "`n") | Where-Object { $_ -match 'stream timings' } | Set-Content "$Out\host-stream-timings.txt"
            }
        }
    )
} catch {
    # A throw after the hang was seen still ends with exit 3.
    if ($code -ne $EXIT_GPU_HANG) { throw }
    Write-Host "after the hang: $_" -ForegroundColor Red
} finally {
    $code = Invoke-Steps $code @(
        { if ($batch -and -not $batch.HasExited) { Stop-Process -Id $batch.Id -Force -ErrorAction SilentlyContinue; Stop-RemoteBatch } }
        { if ($hostProcess -and -not $hostProcess.HasExited) { Stop-Process -Id $hostProcess.Id -Force } }
        { if ($watch -and -not $watch.HasExited) { Stop-Process -Id $watch.Id -Force -ErrorAction SilentlyContinue } }
        { if ($lockWritten) { Lock-Release $script:CheckGpuLock $lockWritten } }
        { if (-not $Keep) { ssh -n nuc "docker exec nj-gamescope rm -rf $Work/fphome; W=$Work bash $Work/tools/container.sh rm" } }
    )
}
exit $code
