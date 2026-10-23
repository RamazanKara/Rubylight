# PyroWave desktop decode measurement protocol (docs/pyrowave-decode-stages.md) with a Linux client:
# an isolated Rubylight host on this PC streams a scrolling texture over the LAN to Nightjar
# running headless on a Linux machine (scripts/pyrowave-linux-leg.sh over ssh), 1080p60 PyroWave,
# in A B B A order per bitrate, one timing CSV per leg, summarised by timing_summary.py.
#
#   .\scripts\pyrowave-linux.ps1 -Remote nuc -RemoteDir '~/nj-g3-run' -DecoderA Cpu -DecoderB Gpu
#
# <RemoteDir> on the Linux machine holds <Bin>/ (nightjar and libpyrowave-shared.so), cfg/ and
# pyrowave-linux-leg.sh (copied by this script). Takes C:\src\butterpollo-release\GPU-LOCK (waits
# while another agent holds it) and refuses to start unless the installed host has no session.
# Exit codes: 0 done, 2 a leg failed, 3 AMD encoder hang suspected (stop and investigate, never
# retry), 4 not idle.
param(
    [string]$Remote = 'nuc',
    [string]$RemoteDir = '~/nj-g3-run',
    [string]$Bin = 'bin',   # the folder of RemoteDir with nightjar (and the SDK library)
    [string]$HostIp = '192.168.4.69',
    [int[]]$Bitrates = @(300000),
    [int]$Seconds = 60,
    [int]$Width = 1920,
    [int]$Height = 1080,
    [int]$Fps = 60,
    [ValidateSet('Cpu', 'Gpu')][string]$DecoderA = 'Cpu',
    [ValidateSet('Cpu', 'Gpu')][string]$DecoderB = 'Gpu',
    [string]$Order = 'ABBA',
    [int]$Port = 49123,
    [string]$Out = ("C:\src\nj-pwd-out\linux-" + (Get-Date -Format 'yyyyMMdd-HHmmss')),
    [string]$HostExe = 'C:\Program Files\Rubylight\butterpollo.exe',
    [string]$HostProfile = 'C:\tests\njg3',
    [string]$Agent = 'nightjar-g3-linux',
    [string]$Metric = 'receive_to_present_ms'
)
$ErrorActionPreference = 'Stop'
$Lock = 'C:\src\butterpollo-release\GPU-LOCK'
$Pin = '1234'
$OutName = Split-Path $Out -Leaf
New-Item -ItemType Directory -Force $Out | Out-Null

function Test-Idle {
    try {
        $info = (Invoke-WebRequest -UseBasicParsing -TimeoutSec 5 http://127.0.0.1:47989/serverinfo).Content
        foreach ($key in 'RustHostSessionCount', 'RustHostPendingSessionCount') {
            if ($info -notmatch "<$key>0</$key>") { Write-Host "not idle: $key is not 0"; return $false }
        }
    } catch {
        $service = Get-Service ApolloService -ErrorAction SilentlyContinue
        if ($service -and $service.Status -ne 'Stopped') { Write-Host "not idle: the installed host does not answer on 47989 ($($_.Exception.Message))"; return $false }
        Write-Host 'no installed host service running; treating the PC as idle'
    }
    $true
}
if (-not (Test-Idle)) { exit 4 }

# The lock is created, not checked then written, so two agents cannot both get it.
$deadline = (Get-Date).AddMinutes(60)
while ($true) {
    try {
        $stream = [IO.File]::Open($Lock, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::Read)
        $writer = [IO.StreamWriter]::new($stream); $writer.Write("$Agent $(Get-Date -Format s)"); $writer.Dispose()
        break
    } catch [IO.IOException] {
        if ((Get-Date) -gt $deadline) { throw "GPU-LOCK still held after 60 minutes: $(Get-Content $Lock -Raw -ErrorAction SilentlyContinue)" }
        if (-not $said) { Write-Host "waiting for GPU-LOCK ($(Get-Content $Lock -Raw -ErrorAction SilentlyContinue))"; $said = $true }
        Start-Sleep -Milliseconds 500
    }
}
if (-not (Test-Idle)) { Remove-Item $Lock -Force; exit 4 }

$hostProcess = $null; $anim = $null
try {
    # --- Host profile, kept between runs so the pairing stays ---------------------------------
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
        -ArgumentList '--config-dir', $HostProfile, '--assets', "`"$install\assets\web`"", '--port', $Port, '--bind', $HostIp, '--no-tray'
    $up = $false
    foreach ($i in 1..60) {
        try { Invoke-WebRequest -UseBasicParsing -TimeoutSec 2 "http://${HostIp}:$Port/serverinfo" | Out-Null; $up = $true; break } catch { Start-Sleep -Seconds 1 }
    }
    if (-not $up) { throw "test host did not answer on ${HostIp}:$Port" }

    # --- Client side (over ssh) -------------------------------------------------------------
    ssh $Remote "mkdir -p $RemoteDir/cfg $RemoteDir/out-$OutName"
    scp -q "$PSScriptRoot\pyrowave-linux-leg.sh" "${Remote}:$RemoteDir/pyrowave-linux-leg.sh"
    if ($LASTEXITCODE) { throw 'scp failed' }
    function Set-ClientSettings([int]$Kbps, [string]$Decoder) {
        $json = (@{ width = $Width; height = $Height; fps = $Fps; bitrate_kbps = $Kbps; pyrowave_decoder = $Decoder; adaptive_bitrate = $false; codec = 'PyroWave';
            window_mode = 'Windowed'; overlay = $true; console_home = $false; early_decode = $false } | ConvertTo-Json -Compress)
        $local = Join-Path $Out 'settings.json'
        [IO.File]::WriteAllText($local, $json)
        scp -q $local "${Remote}:$RemoteDir/cfg/settings.json"
        if ($LASTEXITCODE) { throw 'scp failed' }
    }
    function Start-Leg([string]$Name, [int]$StreamSeconds) {
        Start-Process ssh -PassThru -WindowStyle Hidden -RedirectStandardOutput "$Out\$Name.ssh.log" `
            -ArgumentList $Remote, "`"bash $RemoteDir/pyrowave-linux-leg.sh $RemoteDir/$Bin $RemoteDir/cfg $RemoteDir/out-$OutName $Name $StreamSeconds ${HostIp}:$Port $Pin`""
    }

    $paired = ssh $Remote "grep -c BEGIN $RemoteDir/cfg/hosts.json 2>/dev/null"
    if (-not ($paired -as [int])) {
        Write-Host 'pairing the client with the test host'
        Set-ClientSettings $Bitrates[0] 'Cpu'
        $p = Start-Leg 'pair' 5
        $null = $p.Handle
        $web = "https://${HostIp}:$($Port + 1)"
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
        if (-not $p.WaitForExit(150000)) { $p.Kill(); throw 'pairing run did not finish' }
        if ($p.ExitCode -ne 0) { throw "pairing run failed (exit $($p.ExitCode)); see $Out\pair.ssh.log" }
    }

    # --- Scripted content -------------------------------------------------------------------
    $legs = @(); foreach ($kbps in $Bitrates) { foreach ($variant in $Order.ToCharArray()) { $legs += , @($variant, $kbps) } }
    $anim = Start-Process pwsh -PassThru -WindowStyle Minimized -ArgumentList '-NoProfile', '-File', "$PSScriptRoot\pyrowave-anim.ps1", '-Seconds', ($legs.Count * ($Seconds + 45) + 120)
    Start-Sleep -Seconds 8
    if ($anim.HasExited) { throw "the scripted content (pyrowave-anim.ps1) did not start (exit $($anim.ExitCode))" }

    # --- Legs -------------------------------------------------------------------------------
    $summaryLegs = @(); $n = 0
    foreach ($leg in $legs) {
        $n++; $variant = [string]$leg[0]; $kbps = [int]$leg[1]; $group = [string][math]::Round($kbps / 1000)
        $name = '{0:00}-{1}-{2}' -f $n, $variant, $group
        $decoder = if ($variant -eq 'B') { $DecoderB } else { $DecoderA }
        Set-ClientSettings $kbps $decoder
        $hostLines = (Get-Content $hostLog -ErrorAction SilentlyContinue | Measure-Object -Line).Lines
        Write-Host "leg $name ($Seconds s at $kbps kbps, ${Width}x${Height}@$Fps, decoder $decoder)"
        $p = Start-Leg $name $Seconds
        $null = $p.Handle
        $ended = $p.WaitForExit(($Seconds + 150) * 1000)
        if (-not $ended) { $p.Kill() }
        $hostText = (Get-Content $hostLog -ErrorAction SilentlyContinue | Select-Object -Skip $hostLines) -join "`n"
        if ($hostText -match '(?i)gpu hang|encoder hang|device removed') {
            Write-Host "leg ${name}: AMD encoder hang suspected, stopping (exit 3). Do not retry; see $hostLog"; exit 3
        }
        if (-not $ended) { Write-Host "leg $name did not end"; exit 2 }
        $sent = [regex]::Matches($hostText, 'sent_mbps=([0-9.]+)') | ForEach-Object { [double]$_.Groups[1].Value } | Sort-Object
        $sentMedian = if ($sent.Count) { $sent[[int][math]::Floor(($sent.Count - 1) / 2)] } else { [double]::NaN }
        "$name host sent_mbps median $([math]::Round($sentMedian, 1)) over $($sent.Count) timing lines" | Tee-Object -FilePath "$Out\sent_mbps.txt" -Append | Write-Host
        scp -q "${Remote}:$RemoteDir/out-$OutName/$name*" $Out
        if ($p.ExitCode -ne 0) { Write-Host "leg $name failed (exit $($p.ExitCode)); see $Out\$name.log"; exit 2 }
        $summaryLegs += "${variant}:${group}:$Out\$name.csv"
        Start-Sleep -Seconds 5
    }

    $summaryArgs = @("$PSScriptRoot\timing_summary.py", '--keep', '--metric', $Metric) + $summaryLegs
    python @summaryArgs *> "$Out\summary.txt"
    Get-Content "$Out\summary.txt"
    Write-Host "results in $Out"
} finally {
    if ($anim -and -not $anim.HasExited) { Stop-Process -Id $anim.Id -Force }
    if ($hostProcess -and -not $hostProcess.HasExited) { Stop-Process -Id $hostProcess.Id -Force }
    if ((Test-Path $Lock) -and ((Get-Content $Lock -Raw) -like "$Agent *")) { Remove-Item $Lock -Force }
}
