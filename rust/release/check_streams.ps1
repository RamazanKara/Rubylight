# check.ps1's locks, quiet-CPU waits and stream runs, dot-sourced (and by
# test_release.py with stand-ins for e2e.py and machine_load.py). Needs
# $python and $tools from the caller.
#
# Parallel sessions on the release workstation share two lock files in
# C:\src\butterpollo-release: BUILD-LOCK (build-lock.ps1: heavy builds; a
# lock unwritten for 45 minutes is broken) and GPU-LOCK (streams and GPU
# tests; takelocks.sh, tools/hw/run.py; never broken). Other sessions' builds
# starved the fixtures: rc.4's check failed twice at 25-100 % CPU and passed
# on a quiet one. So the fixture builds hold BUILD-LOCK, each stream holds
# GPU-LOCK and BUILD-LOCK, and a stream starts only on a quiet CPU. A machine
# without build-lock.ps1 there has no locks: the streams only wait for the CPU.

$EXIT_GPU_HANG = 3

function Initialize-CheckLocks([string] $Directory, [switch] $GpuLockHeld) {
    $script:CheckLocks = Test-Path -LiteralPath (Join-Path $Directory 'build-lock.ps1')
    $script:CheckBuildLock = Join-Path $Directory 'BUILD-LOCK'
    $script:CheckGpuLock = Join-Path $Directory 'GPU-LOCK'
    $script:CheckGpuHeld = [bool] $GpuLockHeld
    $script:CheckAncestors = $null
    # Waiters poll BUILD-LOCK every 15 s (build-lock.ps1) and GPU-LOCK every
    # 3 s (takelocks.sh): a lock released and taken again at once would never
    # give them a turn.
    $script:CheckLockPause = 16
    $script:CheckLockBackoff = 20
    $script:CheckLockPoll = 3
    # One quiet-CPU budget for the whole check: once it is spent, each stream
    # only samples the load once and records it.
    $script:CheckQuietBudget = 900
    if (-not $script:CheckLocks) { Write-Warning "No build-lock.ps1 in $Directory; the streams only wait for a quiet CPU." }
}

function Get-AncestorIds {
    if ($null -eq $script:CheckAncestors) {
        $parents = @{}
        foreach ($process in Get-CimInstance Win32_Process -Property ProcessId, ParentProcessId) {
            $parents[[int] $process.ProcessId] = [int] $process.ParentProcessId
        }
        $ids = [Collections.Generic.List[int]]::new()
        $id = $PID
        while ($id -and -not $ids.Contains($id)) {
            $ids.Add($id)
            if (-not $parents.ContainsKey($id)) { break }
            $id = $parents[$id]
        }
        $script:CheckAncestors = $ids
    }
    $script:CheckAncestors
}

# This process or one that started it holds the lock: tools/hw/run.py's
# batch ("pid N"), or a script that took BUILD-LOCK ("pid=N") before calling
# the check. Taking it again would wait for ourselves.
function Test-LockOurs([string] $Path) {
    $holder = Get-Content -LiteralPath $Path -Raw -ErrorAction SilentlyContinue
    [bool] ($holder -and $holder -match '\bpid[= ](\d+)' -and (Get-AncestorIds) -contains [int] $Matches[1])
}

# Writes the lock if it is free and returns what it wrote, else $null.
function Lock-Try([string] $Path, [string] $Owner) {
    $text = "$Owner $(Get-Date -Format o) pid=$PID"
    try { $file = [IO.File]::Open($Path, 'CreateNew', 'Write', 'None') } catch [IO.IOException] { return $null }
    try {
        $writer = [IO.StreamWriter]::new($file)
        $writer.Write($text)
        $writer.Flush()
    } finally { $file.Dispose() }
    $text
}

# Only the lock this run wrote: a lock broken as stale and taken by another
# session stays theirs.
function Lock-Release([string] $Path, [string] $Written) {
    if ($Written -and (Get-Content -LiteralPath $Path -Raw -ErrorAction SilentlyContinue) -ceq $Written) {
        Remove-Item -LiteralPath $Path -Force
    }
}

# Keeps a long hold of BUILD-LOCK from looking stale to build-lock.ps1.
function Lock-Refresh([string] $Path, [string] $Written) {
    if ($Written -and (Get-Content -LiteralPath $Path -Raw -ErrorAction SilentlyContinue) -ceq $Written) {
        (Get-Item -LiteralPath $Path).LastWriteTime = Get-Date
    }
}

# build-lock.ps1's rule: a lock unwritten for 45 minutes was left behind.
function Remove-StaleBuildLock {
    $item = Get-Item -LiteralPath $script:CheckBuildLock -ErrorAction SilentlyContinue
    if ($item -and ((Get-Date) - $item.LastWriteTime).TotalMinutes -gt 45) {
        Remove-Item -LiteralPath $script:CheckBuildLock -Force -ErrorAction SilentlyContinue
        return $true
    }
    $false
}

function Write-Holder([string] $What, [string] $Path) {
    $holder = Get-Content -LiteralPath $Path -Raw -ErrorAction SilentlyContinue
    if ($holder -and $holder -ne $script:CheckLastHolder) { Write-Host "waiting for $What`: $holder" }
    $script:CheckLastHolder = $holder
}

# For the fixture builds: BUILD-LOCK only, waiting like Enter-BuildLock.
function Enter-CheckBuildLock {
    if (-not $script:CheckLocks -or (Test-LockOurs $script:CheckBuildLock)) { return @{} }
    while ($true) {
        $written = Lock-Try $script:CheckBuildLock 'check'
        if ($written) { return @{ build = $written } }
        if (Remove-StaleBuildLock) { continue }
        Write-Holder 'the build lock' $script:CheckBuildLock
        Start-Sleep -Seconds 15
    }
}

# For a stream: GPU-LOCK first, then BUILD-LOCK without waiting. Waiting for
# BUILD-LOCK while holding GPU-LOCK could deadlock with a session that holds
# BUILD-LOCK and waits for the GPU; with BUILD-LOCK busy the GPU goes back
# for a while and both are tried again.
function Enter-StreamLocks {
    if (-not $script:CheckLocks) { return @{} }
    while ($true) {
        $held = @{}
        if (-not $script:CheckGpuHeld -and -not (Test-LockOurs $script:CheckGpuLock)) {
            while (-not ($held.gpu = Lock-Try $script:CheckGpuLock 'release check.ps1')) {
                Write-Holder 'the GPU lock' $script:CheckGpuLock
                Start-Sleep -Seconds $script:CheckLockPoll
            }
        }
        if (Test-LockOurs $script:CheckBuildLock) { return $held }
        if ($held.build = Lock-Try $script:CheckBuildLock 'check') { return $held }
        if (Remove-StaleBuildLock) {
            Lock-Release $script:CheckGpuLock $held.gpu
            continue
        }
        Lock-Release $script:CheckGpuLock $held.gpu
        Write-Holder 'the build lock (the GPU lock is back for now)' $script:CheckBuildLock
        Start-Sleep -Seconds $script:CheckLockBackoff
    }
}

function Exit-CheckLocks([hashtable] $Held) {
    if (-not $Held) { return }
    Lock-Release $script:CheckBuildLock $Held.build
    Lock-Release $script:CheckGpuLock $Held.gpu
    if ($Held.build -or $Held.gpu) { Start-Sleep -Seconds $script:CheckLockPause }
}

# machine_load.py's verdict goes to $Out for e2e.py to record as quiet_wait.
function Wait-Quiet([string] $Out) {
    & $python "$tools\machine_load.py" --timeout ([math]::Max(0, [int] $script:CheckQuietBudget)) --out $Out
    if ($LASTEXITCODE) { throw "machine_load.py failed with exit code $LASTEXITCODE" }
    $script:CheckQuietBudget -= (Get-Content -LiteralPath $Out -Raw | ConvertFrom-Json).waited_seconds
}

# Exit code 3, as e2e.py's: tools/hw/run.py stops its batch only on that.
function Stop-AtHang([string] $Case, [string] $ResultPath) {
    $hang = (Get-Content -LiteralPath $ResultPath -Raw | ConvertFrom-Json).gpu_hang
    Write-Host "stream $Case stopped at an AMD video engine hang: $hang. Not retried: reboot if Device Manager shows the Radeon with Code 31, then run the check again." -ForegroundColor Red
    exit $EXIT_GPU_HANG
}

function Invoke-StreamCase([string] $Case, [string[]] $Stream, [string] $Run) {
    $quiet = Join-Path $Run "quiet-$Case.json"
    $resultPath = Join-Path $Run "e2e-$Case\result.json"
    $held = Enter-StreamLocks
    try {
        Wait-Quiet $quiet
        Lock-Refresh $script:CheckBuildLock $held.build
        try {
            & $python "$tools\e2e.py" @Stream --quiet-wait $quiet
        } catch {
            # e2e.py stopped at an AMD video engine hang (gpu_hang.py). Windows is
            # resetting the GPU; a retry would stream on it and hang or fail again.
            if ($LASTEXITCODE -eq $EXIT_GPU_HANG) { Stop-AtHang $Case $resultPath }
            # Shared-machine load can starve the tone or motion fixture too. Keep
            # the evidence: a passing retry does not explain the first failure.
            if (Test-Path -LiteralPath $resultPath) {
                $result = Get-Content -LiteralPath $resultPath -Raw | ConvertFrom-Json
                $load = $result.machine_load
                Write-Warning ("stream $Case failed: " + ($result.failures -join '; ') + $(if ($load) {
                    " (beside $($load.cpu_percent) % CPU" + $(if ($load.busy_tools) { ', ' + ($load.busy_tools -join ', ') }) + ')'
                }))
            }
            Write-Warning "stream $Case failed; preserving its logs and retrying once. Check shared CPU/GPU load and tone.log; a second failure stops the check."
            $failed = Join-Path $Run "e2e-$Case-failed"
            if (([IO.Path]::GetFullPath($failed) | Split-Path -Parent) -ne [IO.Path]::GetFullPath($Run)) { throw 'Test output left the work directory' }
            Remove-Item -LiteralPath $failed -Recurse -Force -ErrorAction SilentlyContinue
            # A run refused before it started (a busy installed host) left no logs.
            if (Test-Path -LiteralPath "$Run\e2e-$Case") { Move-Item -LiteralPath "$Run\e2e-$Case" -Destination $failed }
            Lock-Refresh $script:CheckBuildLock $held.build
            Wait-Quiet $quiet
            Lock-Refresh $script:CheckBuildLock $held.build
            try {
                & $python "$tools\e2e.py" @Stream --quiet-wait $quiet
            } catch {
                if ($LASTEXITCODE -eq $EXIT_GPU_HANG) { Stop-AtHang $Case $resultPath }
                throw
            }
        }
    } finally { Exit-CheckLocks $held }
}
