# PyroWave desktop decode measurement protocol (docs/pyrowave-decode-stages.md), end to end:
# an isolated Rubylight host on 127.0.0.1:48123 streams a scrolling texture to Nightjar on the
# same PC, 1080p60 PyroWave 4:2:0, in A B B A order per bitrate, one timing CSV per leg.
#
#   .\scripts\pyrowave-loopback.ps1 -ClientA <parent nightjar.exe> -ClientB <candidate nightjar.exe>
#   .\scripts\pyrowave-loopback.ps1 -ClientA <nightjar.exe>            # baseline stage split only
#   .\scripts\pyrowave-loopback.ps1 -ClientA <a.exe> -ClientB <b.exe> -Codec Hevc -Bitrates 0
#   .\scripts\pyrowave-loopback.ps1 -ClientA <nightjar.exe> -ClientB <nightjar.exe> -DecoderA Cpu -DecoderB Gpu -Width 2560 -Height 1440
#   .\scripts\pyrowave-loopback.ps1 -ClientA <nightjar.exe> -DecoderA Gpu -ExtraA @{ beam_race = $true; window_mode = 'Borderless'; vrr = $false; vsync = $false } -PresentMon <PresentMon.exe>
#
# -PresentMon captures the client's presents with PresentMon 2.x (console build, needs this shell to be
# elevated) into <leg>.presentmon.csv next to the leg's timing CSV; the summary counts its PresentMode
# ("Hardware: Independent Flip" is a direct flip to the display, "Composed: Flip" went through the
# desktop compositor). Without -CaptureVisible the client window is excluded from the host's capture
# (WDA_EXCLUDEFROMCAPTURE), which may itself force composition: run once with -CaptureVisible to compare.
#
# Needs release builds (the MSYS2 UCRT64 runtime DLLs are put on PATH). Takes
# C:\src\butterpollo-release\GPU-LOCK (waits while another agent holds it) and refuses to start
# unless the installed host has no session. Exit codes: 0 done, 2 a leg failed, 3 AMD encoder hang
# suspected (stop and investigate, never retry), 4 not idle.
param(
    [Parameter(Mandatory)][string]$ClientA,
    [string]$ClientB,
    [int[]]$Bitrates = @(133000, 300000),   # 0 is the client's default for the resolution
    [ValidateSet('PyroWave', 'Hevc', 'H264', 'Av1')][string]$Codec = 'PyroWave',
    [int]$Seconds = 60,
    [int]$Width = 1920,
    [int]$Height = 1080,
    [int]$Fps = 60,
    [ValidateSet('Cpu', 'Gpu')][string]$DecoderA = 'Cpu',   # the pyrowave_decoder setting of the A and B legs
    [ValidateSet('Cpu', 'Gpu')][string]$DecoderB = 'Cpu',
    [int]$Port = 48123,
    [string]$Order,
    [string]$Out = ("C:\src\nj-pwd-out\" + (Get-Date -Format 'yyyyMMdd-HHmmss')),
    [string]$HostExe = 'C:\Program Files\Rubylight\butterpollo.exe',
    [string]$HostProfile = 'C:\tests\njdecode',
    [string]$ClientProfile = 'C:\tests\njdecode-client',
    [string]$Agent = 'nightjar-pyrowave-decode',
    [string]$Metric = 'decode_to_present_ms',
    [hashtable]$ExtraA = @{},   # more settings.json keys for the A legs (for example @{ present_at_host_times = $false })
    [hashtable]$ExtraB = @{},   # and for the B legs
    [switch]$EarlyDecode,
    [switch]$KeepLock,   # the lock is already this agent's ("<Agent> ..."): use it and leave it for the next run
    [switch]$CaptureVisible,   # leave the client window in the host's capture (a feedback loop)
    [string]$PresentMon = $env:NIGHTJAR_PRESENTMON,   # PresentMon 2.x console exe: capture the client's presents per leg
    [string]$DumpDir,
    [int]$ClientX = 3560,
    [int]$ClientY = 300
)
$ErrorActionPreference = 'Stop'
$Lock = 'C:\src\butterpollo-release\GPU-LOCK'
$Pin = '1234'
if (-not $Order) { $Order = if ($ClientB) { 'ABBA' } else { 'A' } }
if ($Order -match 'B' -and -not $ClientB) { throw 'Order names B but -ClientB is missing' }
if ($PresentMon -and -not (Test-Path $PresentMon)) { throw "PresentMon not found at $PresentMon" }
New-Item -ItemType Directory -Force $Out | Out-Null

# --- Idle check and GPU lock ----------------------------------------------------------------
try {
    $info = (Invoke-WebRequest -UseBasicParsing -TimeoutSec 5 http://127.0.0.1:47989/serverinfo).Content
    foreach ($key in 'RustHostSessionCount', 'RustHostPendingSessionCount') {
        if ($info -notmatch "<$key>0</$key>") { Write-Host "not idle: $key is not 0"; exit 4 }
    }
} catch {
    # A running installed host that does not answer may be mid-stream or hung: not idle. Without
    # the service (or with it stopped) there is no installed host to disturb.
    $service = Get-Service ApolloService -ErrorAction SilentlyContinue
    if ($service -and $service.Status -ne 'Stopped') {
        Write-Host "not idle: the installed host (service $($service.Status)) does not answer on 47989 ($($_.Exception.Message))"; exit 4
    }
    Write-Host 'no installed host service running; treating the PC as idle'
}

# The lock is created, not checked then written, so two agents cannot both get it.
$deadline = (Get-Date).AddMinutes(60)
while ($true) {
    if ($KeepLock -and (Test-Path $Lock) -and ((Get-Content $Lock -Raw) -like "$Agent *")) { break }
    try {
        $stream = [IO.File]::Open($Lock, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::Read)
        $writer = [IO.StreamWriter]::new($stream)
        $writer.Write("$Agent $(Get-Date -Format s)")
        $writer.Dispose()
        break
    } catch [IO.IOException] {
        if ((Get-Date) -gt $deadline) { throw "GPU-LOCK still held after 60 minutes: $(Get-Content $Lock -Raw -ErrorAction SilentlyContinue)" }
        if (-not $said) { Write-Host "waiting for GPU-LOCK ($(Get-Content $Lock -Raw -ErrorAction SilentlyContinue))"; $said = $true }
        Start-Sleep -Milliseconds 500
    }
}
# Another agent may have started a stream while we waited: look again, now that the lock is ours.
try {
    $info = (Invoke-WebRequest -UseBasicParsing -TimeoutSec 5 http://127.0.0.1:47989/serverinfo).Content
    foreach ($key in 'RustHostSessionCount', 'RustHostPendingSessionCount') {
        if ($info -notmatch "<$key>0</$key>") { Remove-Item $Lock -Force; Write-Host "not idle after the lock: $key"; exit 4 }
    }
} catch { }

$hostProcess = $null; $anim = $null
try {
    # --- Host profile, kept between runs so the pairing stays -------------------------------
    New-Item -ItemType Directory -Force "$HostProfile\record" | Out-Null
    @("port = $Port", 'encoder = amf', 'capture = wgc',
      'virtual_display_layout = extended', 'virtual_display_mode = disabled',
      'dd_configuration_option = disabled', 'dd_resolution_option = disabled',
      'dd_refresh_rate_option = disabled', 'dd_hdr_option = disabled', 'dd_always_restore_from_golden = false',
      'frame_limiter_enable = false', 'stream_audio = false', 'upnp = false', 'enable_discovery = false',
      'vulkan_hdr_layer = false', 'system_tray = false', 'update_check_interval = 0', 'pyrowave = true',
      'lan_encryption_mode = 2') | Set-Content "$HostProfile\sunshine.conf"
    '{"apps": [{"name": "Desktop", "cmd": "", "virtual-display": false}]}' | Set-Content "$HostProfile\apps.json"
    if (-not (Test-Path "$HostProfile\sunshine_state.json")) {
        $salt = -join ((1..8) | ForEach-Object { '{0:x2}' -f (Get-Random -Maximum 256) })
        $hash = [Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes('rust-smoke-only' + $salt))
        [array]::Reverse($hash)
        (@{ username = 'test'; salt = $salt; password = ([BitConverter]::ToString($hash) -replace '-', '') } | ConvertTo-Json -Compress) |
            Set-Content "$HostProfile\sunshine_state.json"
    }
    $install = Split-Path $HostExe
    $env:NO_PROXY = '*'; $env:RUST_LOG = 'info'
    $hostLog = "$HostProfile\logs\butterpollo.log"
    $hostProcess = Start-Process $HostExe -PassThru -WindowStyle Hidden -WorkingDirectory $HostProfile `
        -RedirectStandardOutput "$HostProfile\host.stdout.log" -RedirectStandardError "$HostProfile\host.stderr.log" `
        -ArgumentList '--config-dir', $HostProfile, '--assets', "`"$install\assets\web`"", '--port', $Port, '--bind', '127.0.0.1', '--no-tray'
    $up = $false
    foreach ($i in 1..60) {
        try { Invoke-WebRequest -UseBasicParsing -TimeoutSec 2 "http://127.0.0.1:$Port/serverinfo" | Out-Null; $up = $true; break } catch { Start-Sleep -Seconds 1 }
    }
    if (-not $up) { throw "test host did not answer on $Port" }

    # --- Client side ------------------------------------------------------------------------
    New-Item -ItemType Directory -Force $ClientProfile | Out-Null
    $env:PATH = 'C:\msys64\ucrt64\bin;' + $env:PATH
    function Set-ClientSettings([int]$Kbps, [string]$Decoder = 'Cpu', [hashtable]$Extra = @{}) {
        $settings = @{ width = $Width; height = $Height; fps = $Fps; bitrate_kbps = $Kbps; pyrowave_decoder = $Decoder; adaptive_bitrate = $false; codec = $Codec;
            window_mode = 'Windowed'; overlay = $true; console_home = $false; early_decode = $false }
        foreach ($key in $Extra.Keys) { $settings[$key] = $Extra[$key] }
        ($settings | ConvertTo-Json -Compress) |
            Set-Content "$ClientProfile\settings.json"
    }
    function Start-Client([string]$Exe, [string]$Name, [int]$StreamSeconds, [bool]$Early) {
        $env:NIGHTJAR_CONFIG_DIR = $ClientProfile
        $env:NIGHTJAR_TIMING_CSV = "$Out\$Name.csv"
        $env:NIGHTJAR_EARLY_DECODE = if ($Early) { '1' } else { '0' }
        # One PC, one display: the host captures the display the client window is on, so the window
        # is kept out of the capture (the client's presents would change the host's capture rate).
        if ($CaptureVisible) { Remove-Item Env:\NIGHTJAR_EXCLUDE_FROM_CAPTURE -ErrorAction SilentlyContinue } else { $env:NIGHTJAR_EXCLUDE_FROM_CAPTURE = '1' }
        $env:NIGHTJAR_AUTOSTREAM = "127.0.0.1:$Port"
        $env:NIGHTJAR_AUTOSTREAM_APP = 'Desktop'
        $env:NIGHTJAR_AUTOSTREAM_PIN = $Pin
        $env:NIGHTJAR_AUTOSTREAM_SECONDS = "$StreamSeconds"
        if ($DumpDir) { $env:NIGHTJAR_PYROWAVE_DUMP = $DumpDir } else { Remove-Item Env:\NIGHTJAR_PYROWAVE_DUMP -ErrorAction SilentlyContinue }
        $started = Start-Process $Exe -PassThru -RedirectStandardError "$Out\$Name.log" -RedirectStandardOutput "$Out\$Name.stdout.log"
        $null = $started.Handle  # without a cached handle ExitCode reads empty after the exit
        $started
    }
    # PresentMon follows the client process by exe name and ends with it; the timer is a backstop.
    function Start-PresentMon([string]$Exe, [string]$Name, [int]$StreamSeconds) {
        $session = "nightjar-$Name"
        Start-Process $PresentMon -PassThru -WindowStyle Hidden -RedirectStandardOutput "$Out\$Name.presentmon.stdout.log" -RedirectStandardError "$Out\$Name.presentmon.stderr.log" -ArgumentList `
            '--process_name', (Split-Path $Exe -Leaf), '--output_file', "`"$Out\$Name.presentmon.csv`"", '--no_console_stats', '--session_name', $session,
            '--stop_existing_session', '--terminate_on_proc_exit', '--timed', ($StreamSeconds + 60), '--terminate_after_timed'
    }
    function Move-ClientWindow($Process) {
        Add-Type -Namespace Win -Name Native -ErrorAction SilentlyContinue -MemberDefinition '[DllImport("user32.dll")] public static extern bool MoveWindow(System.IntPtr h, int x, int y, int w, int hh, bool r);'
        foreach ($i in 1..40) {
            $Process.Refresh()
            if ($Process.MainWindowHandle -ne 0) { [void][Win.Native]::MoveWindow($Process.MainWindowHandle, $ClientX, $ClientY, 1296, 840, $true); return }
            Start-Sleep -Milliseconds 250
        }
    }

    # Pair once: the client asks with PIN 1234, this posts it to the test host's web UI.
    $paired = (Test-Path "$ClientProfile\hosts.json") -and ((Get-Content "$ClientProfile\hosts.json" -Raw) -match '"server_certificate":\s*"-----BEGIN')
    if (-not $paired) {
        Write-Host 'pairing the client with the test host'
        Set-ClientSettings $Bitrates[0]
        $p = Start-Client $ClientA 'pair' 5 $false
        $web = "https://127.0.0.1:$($Port + 1)"
        $login = Invoke-RestMethod -SkipCertificateCheck -SessionVariable session -Method Post "$web/api/auth/login" `
            -ContentType 'application/json' -Body (@{ username = 'test'; password = 'rust-smoke-only' } | ConvertTo-Json)
        $headers = @{ 'X-CSRF-Token' = $login.csrf_token }
        $posted = $false
        foreach ($i in 1..60) {
            if ($p.HasExited) { break }
            $pending = Invoke-RestMethod -SkipCertificateCheck -WebSession $session -Headers $headers "$web/api/clients/pending"
            foreach ($request in $pending.requests) {
                Invoke-RestMethod -SkipCertificateCheck -WebSession $session -Headers $headers -Method Post "$web/api/pin" -ContentType 'application/json' `
                    -Body (@{ pin = $Pin; name = $request.name; uniqueid = $request.uniqueid } | ConvertTo-Json) | Out-Null
                $posted = $true
            }
            if ($posted) { break }
            Start-Sleep -Seconds 1
        }
        if (-not $p.WaitForExit(120000)) { $p.Kill(); throw 'pairing run did not finish' }
        if ($p.ExitCode -ne 0) { throw "pairing run failed (exit $($p.ExitCode)); see $Out\pair.log" }
    }

    # --- Scripted content -------------------------------------------------------------------
    $legs = @(); foreach ($kbps in $Bitrates) { foreach ($variant in $Order.ToCharArray()) { $legs += , @($variant, $kbps) } }
    $animSeconds = $legs.Count * ($Seconds + 45) + 120
    $anim = Start-Process pwsh -PassThru -WindowStyle Minimized -ArgumentList '-NoProfile', '-File', "$PSScriptRoot\pyrowave-anim.ps1", '-Seconds', $animSeconds
    Start-Sleep -Seconds 8
    if ($anim.HasExited) { throw "the scripted content (pyrowave-anim.ps1) did not start (exit $($anim.ExitCode)); without it the host captures a still desktop" }

    # --- Legs -------------------------------------------------------------------------------
    $summaryLegs = @(); $n = 0
    foreach ($leg in $legs) {
        $n++; $variant = [string]$leg[0]; $kbps = [int]$leg[1]; $group = if ($kbps -eq 0) { 'default' } else { [string][math]::Round($kbps / 1000) }
        $name = '{0:00}-{1}-{2}' -f $n, $variant, $group
        $exe = if ($variant -eq 'B') { $ClientB } else { $ClientA }
        Set-ClientSettings $kbps $(if ($variant -eq 'B') { $DecoderB } else { $DecoderA }) $(if ($variant -eq 'B') { $ExtraB } else { $ExtraA })
        $hostLines = (Get-Content $hostLog -ErrorAction SilentlyContinue | Measure-Object -Line).Lines
        Write-Host "leg $name ($Seconds s at $kbps kbps, ${Width}x${Height}@$Fps, decoder $(if ($variant -eq 'B') { $DecoderB } else { $DecoderA }))"
        $p = Start-Client $exe $name $Seconds $EarlyDecode.IsPresent
        $extra = if ($variant -eq 'B') { $ExtraB } else { $ExtraA }
        # A fullscreen leg stays where it is; only a window is put at the corner of the second display.
        if (-not $extra.ContainsKey('window_mode') -or $extra['window_mode'] -eq 'Windowed') { Move-ClientWindow $p }
        $meter = if ($PresentMon) { Start-PresentMon $exe $name $Seconds }
        $ended = $p.WaitForExit(($Seconds + 120) * 1000)
        if (-not $ended) { $p.Kill() }
        if ($meter) {
            if (-not $meter.WaitForExit(20000)) { Stop-Process -Id $meter.Id -Force; Write-Host "leg ${name}: PresentMon did not end by itself" }
            if (-not (Test-Path "$Out\$name.presentmon.csv")) { Write-Host "leg ${name}: PresentMon wrote no CSV; see $Out\$name.presentmon.stderr.log"; exit 2 }
        }
        # A hang is looked for first: a leg that stalls or fails because the encoder hung is exit 3, not 2.
        $hostText = (Get-Content $hostLog -ErrorAction SilentlyContinue | Select-Object -Skip $hostLines) -join "`n"
        if ($hostText -match '(?i)gpu hang|encoder hang|device removed') {
            Write-Host "leg ${name}: AMD encoder hang suspected, stopping (exit 3). Do not retry; see $hostLog"; exit 3
        }
        if (-not $ended) { Write-Host "leg $name did not end"; exit 2 }
        $sent = [regex]::Matches($hostText, 'sent_mbps=([0-9.]+)') | ForEach-Object { [double]$_.Groups[1].Value } | Sort-Object
        $sentMedian = if ($sent.Count) { $sent[[int][math]::Floor(($sent.Count - 1) / 2)] } else { [double]::NaN }
        "$name host sent_mbps median $([math]::Round($sentMedian, 1)) over $($sent.Count) timing lines" | Tee-Object -FilePath "$Out\sent_mbps.txt" -Append | Write-Host
        if ($p.ExitCode -ne 0) { Write-Host "leg $name failed (exit $($p.ExitCode)); see $Out\$name.log"; exit 2 }
        $summaryLegs += "${variant}:${group}:$Out\$name.csv"
        Start-Sleep -Seconds 5
    }

    $keep = if ($ClientB) { @('--keep', '--metric', $Metric) } else { @() }
    $summaryArgs = @("$PSScriptRoot\timing_summary.py") + $keep + $summaryLegs
    python @summaryArgs *> "$Out\summary.txt"
    Get-Content "$Out\summary.txt"
    Write-Host "results in $Out"
} finally {
    if ($anim -and -not $anim.HasExited) { Stop-Process -Id $anim.Id -Force }
    if ($hostProcess -and -not $hostProcess.HasExited) { Stop-Process -Id $hostProcess.Id -Force }
    if (-not $KeepLock -and (Test-Path $Lock) -and ((Get-Content $Lock -Raw) -like "$Agent *")) { Remove-Item $Lock -Force }
}
