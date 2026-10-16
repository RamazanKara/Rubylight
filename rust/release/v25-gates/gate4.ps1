# Task B gate 4 on this PC (elevated, installed host idle):
#   1. forced below-floor verdicts (windows, gpu, driver) each refuse and change nothing;
#   2. an upgrade from the installed release keeps settings, pairings and the app library, and
#      migrates the removed settings the profile carries (capture = wgc, sw_preset, ...);
#   3. an isolated copy of the profile naming a removed encoder starts, migrates to AMF and is
#      written back once.
# usage: gate4.ps1 -Setup <2.5 setup.exe> -FloorSetup <floor-test setup.exe> -Package <2.5 package dir> -Out <dir>
param(
    [Parameter(Mandatory)] [string] $Setup,
    [Parameter(Mandatory)] [string] $FloorSetup,
    [Parameter(Mandatory)] [string] $Package,
    [Parameter(Mandatory)] [string] $Out
)
$ErrorActionPreference = 'Stop'
$install = 'C:\Program Files\Rubylight'
$profile = 'C:\ProgramData\Butterpollo\config'
New-Item -ItemType Directory -Force $Out | Out-Null
$failures = [System.Collections.Generic.List[string]]::new()

function Run-Setup([string]$Exe, [string[]]$Arguments) {
    $started = Get-Date
    $p = Start-Process -FilePath $Exe -ArgumentList $Arguments -PassThru
    $null = $p.Handle
    $p.WaitForExit()
    $log = Get-ChildItem $env:TEMP -Filter 'butterpollo-setup-*.log' | Where-Object LastWriteTime -ge $started.AddSeconds(-2) | Sort-Object LastWriteTime | Select-Object -Last 1
    [pscustomobject]@{ code = $p.ExitCode; text = if ($log) { Get-Content -Raw $log.FullName } else { '' }; log = $log.FullName }
}

function Assert-Idle {
    $info = [xml](Invoke-WebRequest -UseBasicParsing -NoProxy 'http://127.0.0.1:47989/serverinfo' -TimeoutSec 5).Content
    $root = $info.root
    $busy = [int]("0$($root.RustHostSessionCount)") + [int]("0$($root.RustHostPendingSessionCount)")
    if ($busy -ne 0 -or $root.state -ne 'SUNSHINE_SERVER_FREE') { throw "the installed host is streaming (sessions $busy, state $($root.state))" }
}

function Snapshot {
    $files = Get-ChildItem -LiteralPath $install -File -Recurse | Sort-Object FullName | ForEach-Object {
        "$($_.FullName.Substring($install.Length)) $((Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash)" }
    $config = 'sunshine.conf', 'sunshine_state.json', 'apps.json' | ForEach-Object {
        "$_ $((Get-FileHash -LiteralPath (Join-Path $profile $_) -Algorithm SHA256).Hash)" }
    $driver = (pnputil /enum-devices /deviceid 'Root\RubylightVDisplay' /drivers | Select-String 'Driver Version|Status|Driver Name') -join '; '
    $service = (Get-Service ApolloService | ForEach-Object { "$($_.Status) $($_.StartType)" })
    $uninstall = Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*' | Where-Object DisplayName -eq 'Rubylight' | ForEach-Object { "$($_.DisplayVersion)" }
    [pscustomobject]@{ files = $files; config = $config; driver = $driver; service = $service; version = $uninstall }
}

function Same($a, $b, $what) {
    foreach ($field in 'files', 'config', 'driver', 'service', 'version') {
        $x = ($a.$field) -join "`n"; $y = ($b.$field) -join "`n"
        if ($x -ne $y) { $failures.Add("${what}: $field changed") }
    }
}

function Profile-Facts {
    $state = Get-Content -Raw (Join-Path $profile 'sunshine_state.json') | ConvertFrom-Json
    $devices = @($state.root.named_devices | ForEach-Object { $_.uuid }) | Sort-Object
    $apps = (Get-Content -Raw (Join-Path $profile 'apps.json') | ConvertFrom-Json).apps | ForEach-Object { $_.name }
    $settings = [ordered]@{}
    foreach ($line in Get-Content (Join-Path $profile 'sunshine.conf')) {
        if ($line -match '^\s*([^#=\s][^=]*?)\s*=\s*(.*)$') { $settings[$Matches[1]] = $Matches[2] }
    }
    [pscustomobject]@{ devices = $devices; apps = @($apps); settings = $settings }
}

# 1. Forced below-floor verdicts.
Assert-Idle
$before = Snapshot
foreach ($part in 'windows', 'gpu', 'driver') {
    Assert-Idle
    $env:RUBYLIGHT_SETUP_FLOOR_TEST = $part
    $r = Run-Setup $FloorSetup @('--quiet')
    Remove-Item Env:\RUBYLIGHT_SETUP_FLOOR_TEST
    Copy-Item $r.log (Join-Path $Out "floor-$part.log")
    $p = [pscustomobject]@{ ExitCode = $r.code }; $text = $r.text
    "floor $part exit $($p.ExitCode)" | Tee-Object -Append (Join-Path $Out 'gate4.txt')
    if ($p.ExitCode -eq 0) { $failures.Add("floor ${part}: setup exited 0") }
    if ($text -notmatch '2\.4\.5') { $failures.Add("floor ${part}: the refusal does not name 2.4.5 as the last version") }
    Start-Sleep -Seconds 3
    Same $before (Snapshot) "floor $part"
}

# 2. Upgrade over the installed release.
Assert-Idle
$old = Profile-Facts
Copy-Item -LiteralPath (Join-Path $profile 'sunshine.conf') -Destination (Join-Path $Out 'sunshine.conf.before')
$r = Run-Setup $Setup @('--quiet'); Copy-Item $r.log (Join-Path $Out 'upgrade.log'); $p = [pscustomobject]@{ ExitCode = $r.code }
"upgrade exit $($p.ExitCode)" | Tee-Object -Append (Join-Path $Out 'gate4.txt')
if ($p.ExitCode -ne 0) { $failures.Add("upgrade: setup exited $($p.ExitCode)") }
$deadline = (Get-Date).AddSeconds(90)
do {
    Start-Sleep -Seconds 2
    try { $ready = (Invoke-WebRequest -UseBasicParsing -NoProxy 'http://127.0.0.1:47989/serverinfo' -TimeoutSec 3).StatusCode -eq 200 } catch { $ready = $false }
} until ($ready -or (Get-Date) -gt $deadline)
if (-not $ready) { $failures.Add('upgrade: the host did not come back') }
$new = Profile-Facts
Copy-Item -LiteralPath (Join-Path $profile 'sunshine.conf') -Destination (Join-Path $Out 'sunshine.conf.after')
if (($old.devices -join ',') -ne ($new.devices -join ',')) { $failures.Add('upgrade: paired devices changed') }
if (($old.apps -join ',') -ne ($new.apps -join ',')) { $failures.Add('upgrade: the app library changed') }
$retired = '^(frame_limiter_disable_vsync|rtss_disable_vsync_ullm|rtx_hdr.*|dd_wa_dummy_plug_hdr10|min_threads|crf|qp|capture|wgc_.*|capture_predictive_poll|sw_.*|nvenc_.*|qsv_.*|virtual_display_driver|dd_use_sunshine_virtual_display_driver|output_name|gpu_compute_conversion)$'
foreach ($key in $old.settings.Keys) {
    if ($key -match $retired) {
        if ($new.settings.Contains($key)) { $failures.Add("upgrade: retired setting $key was kept") }
    } elseif (-not $new.settings.Contains($key)) {
        $failures.Add("upgrade: setting $key was lost")
    } elseif ($new.settings[$key] -ne $old.settings[$key] -and $key -notin 'rtss_frame_limit_type', 'frame_limiter_provider') {
        $failures.Add("upgrade: setting $key changed from $($old.settings[$key]) to $($new.settings[$key])")
    }
}
$version = (Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*' | Where-Object DisplayName -eq 'Rubylight').DisplayVersion
"installed version $version" | Tee-Object -Append (Join-Path $Out 'gate4.txt')
$hostLog = Get-ChildItem (Join-Path $profile 'logs') -Filter 'butterpollo*.log' | Sort-Object LastWriteTime | Select-Object -Last 1
if ($hostLog) { Select-String -LiteralPath $hostLog.FullName -Pattern 'retired|settings_retired|was removed' | Select-Object -Last 5 | ForEach-Object { "host: $($_.Line)" } | Tee-Object -Append (Join-Path $Out 'gate4.txt') }

# 3. A removed encoder in an isolated copy of the profile.
$copy = Join-Path $Out 'profile-copy'
if (Test-Path $copy) { Remove-Item -Recurse -Force $copy }
New-Item -ItemType Directory $copy | Out-Null
Copy-Item (Join-Path $Out 'sunshine.conf.before') (Join-Path $copy 'sunshine.conf')
Copy-Item (Join-Path $profile 'apps.json') $copy
(Get-Content (Join-Path $copy 'sunshine.conf')) -replace '^encoder = .*', 'encoder = nvenc' | Set-Content (Join-Path $copy 'sunshine.conf')
Add-Content (Join-Path $copy 'sunshine.conf') "nvenc_preset = 4`nqsv_preset = slow`ngpu_compute_conversion = false`nport = 48523"
$h = Start-Process -FilePath (Join-Path $Package 'butterpollo.exe') -ArgumentList '--config-dir', $copy, '--assets', (Join-Path $Package 'assets\web'), '--bind', '127.0.0.1', '--no-tray' -PassThru -WindowStyle Hidden -RedirectStandardOutput (Join-Path $Out 'copy-host.log') -RedirectStandardError (Join-Path $Out 'copy-host.err')
$deadline = (Get-Date).AddSeconds(60); $up = $false
do { Start-Sleep -Seconds 1; try { $up = (Invoke-WebRequest -UseBasicParsing -NoProxy 'http://127.0.0.1:48523/serverinfo' -TimeoutSec 2).StatusCode -eq 200 } catch {} } until ($up -or $h.HasExited -or (Get-Date) -gt $deadline)
if (-not $up) { $failures.Add('migration: the host with removed settings did not start') }
Stop-Process -Id $h.Id -Force -ErrorAction SilentlyContinue
$migrated = Get-Content (Join-Path $copy 'sunshine.conf')
if (-not ($migrated -match '^encoder = amf$')) { $failures.Add('migration: encoder nvenc was not replaced by amf') }
if ($migrated -match '^(nvenc_|qsv_|gpu_compute_conversion|capture =|sw_)') { $failures.Add("migration: removed settings remain: $(($migrated -match '^(nvenc_|qsv_|gpu_compute_conversion|capture =|sw_)') -join ', ')") }

$result = [pscustomobject]@{ passed = $failures.Count -eq 0; failures = $failures; version = $version }
$result | ConvertTo-Json | Set-Content (Join-Path $Out 'result.json')
$result | ConvertTo-Json
