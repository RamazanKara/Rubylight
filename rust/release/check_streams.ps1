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
    # GPU-LOCK is never broken as stale: a holder past this ends the check.
    $script:CheckGpuDeadline = 1800
    $script:CheckGpuWarning = 60
    # Locks this run wrote that another session removed or took over.
    $script:CheckLockLost = [Collections.Generic.List[string]]::new()
    if (-not $script:CheckLocks) { Write-Warning "No build-lock.ps1 in $Directory; the streams only wait for a quiet CPU." }
    # Walked once, before any lock is taken.
    [void] (Get-AncestorIds)
}

function Get-AncestorIds {
    if ($null -eq $script:CheckAncestors) {
        $parents = @{}
        foreach ($process in Get-CimInstance Win32_Process -Property ProcessId, ParentProcessId) {
            $parents[[int] $process.ProcessId] = [int] $process.ParentProcessId
        }
        $ids = [Collections.Generic.List[int]]::new()
        $id = $PID
        # A parent that exited left its id to be reused: only running ones count.
        while ($id -and -not $ids.Contains($id) -and $parents.ContainsKey($id)) {
            $ids.Add($id)
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
    $holder = Read-Lock $Path
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

# Another session reading a lock (Python's read_text) holds it open without
# delete sharing for a moment: retry for about a second.
function Invoke-LockFile([scriptblock] $Action) {
    for ($try = 1; ; $try++) {
        try { return & $Action } catch [IO.IOException] {
            if ($try -ge 20) { throw }
            Start-Sleep -Milliseconds 50
        }
    }
}

function Read-Lock([string] $Path) {
    Invoke-LockFile { if ([IO.File]::Exists($Path)) { [IO.File]::ReadAllText($Path) } }
}

# A lock this run wrote that is gone or holds another session's text: said
# loudly, and Invoke-StreamCase adds it to the case's result.json.
function Test-LockStillOurs([string] $Path, [string] $Written) {
    $now = Read-Lock $Path
    if ($now -ceq $Written) { return $true }
    $lost = "$(Split-Path -Leaf $Path) written as '$Written' " + $(if ($null -eq $now) { 'was removed by another session' } else { "was taken over: $now" })
    if (-not $script:CheckLockLost.Contains($lost)) {
        $script:CheckLockLost.Add($lost)
        Write-Host "LOCK LOST: $lost. Another session may have streamed or built beside this check." -ForegroundColor Red
    }
    $false
}

# Only the lock this run wrote: a lock broken as stale and taken by another
# session stays theirs.
function Lock-Release([string] $Path, [string] $Written) {
    if ($Written -and (Test-LockStillOurs $Path $Written)) {
        Invoke-LockFile { [IO.File]::Delete($Path) }
    }
}

# Keeps a long hold of BUILD-LOCK from looking stale to build-lock.ps1.
function Lock-Refresh([string] $Path, [string] $Written) {
    if ($Written -and (Test-LockStillOurs $Path $Written)) {
        Invoke-LockFile { [IO.File]::SetLastWriteTime($Path, (Get-Date)) }
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

# Between the fixture builds, so a long one never looks stale.
function Update-CheckBuildLock([hashtable] $Held) {
    if ($Held) { Lock-Refresh $script:CheckBuildLock $Held.build }
}

# For a stream: GPU-LOCK first, then BUILD-LOCK without waiting. Waiting for
# BUILD-LOCK while holding GPU-LOCK could deadlock with a session that holds
# BUILD-LOCK and waits for the GPU; with BUILD-LOCK busy the GPU goes back
# for a while and both are tried again.
function Enter-StreamLocks {
    if (-not $script:CheckLocks) { return @{} }
    while ($true) {
        $held = @{}
        try {
            if (-not $script:CheckGpuHeld -and -not (Test-LockOurs $script:CheckGpuLock)) {
                # Per GPU wait: time spent waiting for BUILD-LOCK is not the GPU holder's.
                $waiting = [Diagnostics.Stopwatch]::StartNew()
                $warned = 0
                while (-not ($held.gpu = Lock-Try $script:CheckGpuLock 'release check.ps1')) {
                    Write-Holder 'the GPU lock' $script:CheckGpuLock
                    $seconds = $waiting.Elapsed.TotalSeconds
                    if ($seconds -ge $script:CheckGpuDeadline) {
                        throw "GPU-LOCK still held after $([math]::Round($seconds)) s: $(Read-Lock $script:CheckGpuLock). Not breaking it: run the check once that session is done."
                    }
                    if ($seconds -ge ($warned + 1) * $script:CheckGpuWarning) {
                        $warned = [math]::Floor($seconds / $script:CheckGpuWarning)
                        Write-Warning "still waiting for the GPU lock after $([math]::Round($seconds)) s: $(Read-Lock $script:CheckGpuLock)"
                    }
                    Start-Sleep -Seconds $script:CheckLockPoll
                }
            }
            if (Test-LockOurs $script:CheckBuildLock) { return $held }
            if ($held.build = Lock-Try $script:CheckBuildLock 'check') { return $held }
            $stale = Remove-StaleBuildLock
        } catch {
            Exit-CheckLocks $held -NoPause
            throw
        }
        Lock-Release $script:CheckGpuLock $held.gpu
        if ($stale) { continue }
        Write-Holder 'the build lock (the GPU lock is back for now)' $script:CheckBuildLock
        Start-Sleep -Seconds $script:CheckLockBackoff
    }
}

# GPU first, each in its own try: a lock that cannot be removed must not
# keep the other one held.
function Exit-CheckLocks([hashtable] $Held, [switch] $NoPause) {
    if (-not $Held) { return }
    foreach ($lock in @(@($script:CheckGpuLock, $Held.gpu), @($script:CheckBuildLock, $Held.build))) {
        try { Lock-Release $lock[0] $lock[1] } catch {
            $script:CheckLockLost.Add("$(Split-Path -Leaf $lock[0]) could not be removed: $_")
            Write-Host "LOCK LEFT BEHIND: $(Split-Path -Leaf $lock[0]) could not be removed: $_. Remove it by hand." -ForegroundColor Red
        }
    }
    if (($Held.build -or $Held.gpu) -and -not $NoPause) { Start-Sleep -Seconds $script:CheckLockPause }
}

# The case's result.json says which of its locks another session took.
function Write-LockLost([string] $ResultPath) {
    if (-not $script:CheckLockLost.Count -or -not (Test-Path -LiteralPath $ResultPath)) { return }
    $result = Get-Content -LiteralPath $ResultPath -Raw | ConvertFrom-Json -AsHashtable
    $result.lock_warnings = @($script:CheckLockLost)
    $result.warnings = @(@($result.warnings | Where-Object { $_ }) + $script:CheckLockLost)
    $result | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath $ResultPath -Encoding utf8NoBOM
    $script:CheckLockLost.Clear()
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
                }) + $(if ($result.warnings) { '. ' + ($result.warnings -join '; ') }))
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
    } finally {
        Exit-CheckLocks $held
        Write-LockLost $resultPath
    }
}
