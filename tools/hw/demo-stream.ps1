# Demo stream for a device arm (Retroid, TV, laptop): a looped game clip instead of a real game.
# Opens the clip paused in a borderless ffplay window behind the user's windows (rust/release/demo_content.py),
# points an isolated test host at that one window (output_name = window:<hwnd>/<pid>@<display>, WGC), and plays the
# clip from its start when the host's capture opens. No display mode, HDR or virtual display change.
#   tools\hw\demo-stream.ps1 -Clip D:\testmedia\xiph-twitch\GTAV.y4m -Seconds 30 -Package <package dir>
#       [-Profile C:\tests\game] [-Port 48623] [-Bind 192.168.4.69] [-Stage <dir>] [-Out <dir>] [-Extra 'key = value',...]
#       [-Wait <seconds for the device's stream to start, 600>]
# The profile is an existing, paired test profile; only its output_name line changes, and it is put back at the end.
# The stage directory needs its firewall rule already (never add one); the package is mirrored into it.
# The caller holds GPU-LOCK (its text starts with -LockOwner). Start the device's stream once READY is printed;
# content.json in -Out has the player's record (real time kept, CPU), the host log is copied beside it.
# The host needs window targets in output_name (Rubylight after 3.0.0-rc.5).
param([Parameter(Mandatory)][string]$Clip, [Parameter(Mandatory)][int]$Seconds, [Parameter(Mandatory)][string]$Package,
    [string]$Profile = 'C:\tests\game', [int]$Port = 48623, [string]$Bind = '192.168.4.69',
    [string]$Stage = 'C:\src\butterpollo-release\p444\hostdir', [string]$Out = '', [string[]]$Extra = @(),
    [string]$At = '0,0', [string]$LockOwner = '', [string]$Log = 'info', [int]$Wait = 600)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).ProviderPath
# Not %TEMP%: a packaged (MSIX) caller sees a private copy of it, so the files would not be where they are printed.
if (-not $Out) { $Out = "C:\src\butterpollo-release\unified\scratch\demo\out\demo-stream-$(Get-Date -Format yyyyMMdd-HHmmss)" }
New-Item -ItemType Directory -Force $Out | Out-Null
if ($LockOwner -and (Get-Content C:\src\butterpollo-release\GPU-LOCK -ErrorAction SilentlyContinue) -notlike "$LockOwner*") { throw 'GPU-LOCK is not ours' }
$info = (Invoke-WebRequest http://127.0.0.1:47989/serverinfo -UseBasicParsing).Content
if ($info -notmatch '<RustHostSessionCount>0<' -or $info -notmatch '<RustHostPendingSessionCount>0<') { throw 'the installed host has a session: not starting' }
if (Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue) { throw "port $Port is in use" }
if (@(Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -like "$Stage\*" }).Count) { throw "a process runs from $Stage" }
$conf = Join-Path $Profile 'sunshine.conf'
if (-not (Test-Path $conf)) { throw "$conf is missing: pair the device with this profile first" }
robocopy $Package $Stage /MIR /NFL /NDL /NJH /NJS /NP | Out-Null
if ($LASTEXITCODE -ge 8) { throw "robocopy failed $LASTEXITCODE" }
Add-Type -AssemblyName System.Windows.Forms
$display = [System.Windows.Forms.Screen]::PrimaryScreen.DeviceName
$hostLog = Join-Path $Profile 'logs\butterpollo.log'
Get-ChildItem (Join-Path $Profile 'logs') -ErrorAction SilentlyContinue | Remove-Item -Force
$original = [IO.File]::ReadAllText($conf)
$player = $null; $hostProcess = $null
$callerEnv = @{ RUST_LOG = $env:RUST_LOG; NO_PROXY = $env:NO_PROXY }
try {
    $player = Start-Process -PassThru -NoNewWindow -FilePath python -ArgumentList @(
        "`"$repo\rust\release\demo_content.py`"", "`"$Clip`"", '--display', "`"$display`"", '--out', "`"$Out`"",
        '--host-log', "`"$hostLog`"", '--seconds', "$Seconds", '--at', $At, '--wait', "$Wait") -RedirectStandardOutput "$Out\player.out" -RedirectStandardError "$Out\player.err"
    $null = $player.Handle  # without a handle taken now, ExitCode reads empty after the exit
    $end = (Get-Date).AddSeconds(20)
    while (-not (Test-Path "$Out\target.txt")) {
        if ($player.HasExited -or (Get-Date) -gt $end) { throw "the clip player did not open: $(Get-Content "$Out\player.err" -Raw)" }
        Start-Sleep -Milliseconds 100
    }
    $target = Get-Content "$Out\target.txt"
    $keys = @('output_name') + @($Extra | ForEach-Object { $_.Split('=', 2)[0].Trim() })
    $lines = @($original -split "`r?`n" | Where-Object { $_ -and $keys -notcontains $_.Split('=', 2)[0].Trim() }) + "output_name = $target" + $Extra
    [IO.File]::WriteAllText($conf, ($lines -join "`n") + "`n")
    $env:RUST_LOG = $Log; $env:NO_PROXY = '*'
    $hostProcess = Start-Process -PassThru -WindowStyle Hidden -FilePath "$Stage\butterpollo.exe" -ArgumentList '--config-dir', $Profile, '--port', "$Port", '--bind', $Bind -RedirectStandardOutput "$Out\host.out" -RedirectStandardError "$Out\host.err"
    $end = (Get-Date).AddSeconds(40)
    while (-not ((Test-Path $hostLog) -and (Select-String -Path $hostLog -Pattern 'encoder capability probe completed' -Quiet))) {
        if ($hostProcess.HasExited) { throw "the host exited $($hostProcess.ExitCode)" }
        if ((Get-Date) -gt $end) { throw 'the host was not ready after 40 s' }
        Start-Sleep -Milliseconds 250
    }
    "READY $(Get-Date -Format T): host pid $($hostProcess.Id) on ${Bind}:$Port streams $target; start the device's stream"
    $player.WaitForExit()
    "content $(Get-Content "$Out\content.json" -Raw -ErrorAction SilentlyContinue)"
    if ($player.ExitCode) { "the clip player failed ($($player.ExitCode)): $(Get-Content "$Out\player.err" -Raw)" }
} finally {
    if ($player -and -not $player.HasExited) {
        Get-CimInstance Win32_Process -Filter "ParentProcessId=$($player.Id)" | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
        Stop-Process -Id $player.Id -Force
    }
    if ($hostProcess -and -not $hostProcess.HasExited) { Stop-Process -Id $hostProcess.Id -Force; $hostProcess.WaitForExit(30000) | Out-Null }
    [IO.File]::WriteAllText($conf, $original)
    # The host's log level and proxy bypass stay out of the caller's session.
    foreach ($name in $callerEnv.Keys) {
        if ($null -eq $callerEnv[$name]) { Remove-Item "env:$name" -ErrorAction Ignore } else { Set-Item "env:$name" $callerEnv[$name] }
    }
    if (Test-Path $hostLog) { Copy-Item $hostLog "$Out\butterpollo.log" }
    "done $(Get-Date -Format T): $Out"
}
