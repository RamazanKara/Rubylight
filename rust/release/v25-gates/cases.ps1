# Runs run.ps1 cases as SYSTEM in session 1, one at a time, each through its own one-shot
# task deleted afterwards. Extended layout (vd_e2e.py). Elevated.
# cases.ps1 -Runs runs-base1 -Build base -Round 1   (12 cases: HEVC/AV1, SDR/HDR, 60/120/240)
param([Parameter(Mandatory)] [string] $Runs, [string] $Build, [int] $Round, [switch] $Reverse, [string] $LabelList)
Add-Type -AssemblyName System.Windows.Forms
$root = 'C:\src\butterpollo-release\fence-ring'
$stage = 'C:\src\butterpollo-release\stage-latency'
$Runs | Set-Content "$root\runs-dir.txt"
$dir = "$root\$Runs"
New-Item -ItemType Directory -Force $dir | Out-Null
Remove-Item "$dir\cases.done" -ErrorAction SilentlyContinue
Start-Transcript "$dir\cases.log" -Append | Out-Null
$Labels = foreach ($fps in 60, 120, 240) { foreach ($codec in 'hevc', 'av1', 'hevc-hdr', 'av1-hdr') { "$codec-$fps-$Build-r$Round" } }
if ($LabelList) { $Labels = $LabelList -split ',' }
if ($Reverse) { [array]::Reverse($Labels) }
$snapshot = { ([System.Windows.Forms.Screen]::AllScreens | ForEach-Object { "$($_.DeviceName) primary=$($_.Primary) $($_.Bounds)" }) -join '; ' }
$before = & $snapshot
"displays before: $before"
try {
    foreach ($label in $Labels) {
        # Never beside the owner's stream or game, nor another isolated host.
        while ($true) {
            try { $info = [xml](Invoke-WebRequest -UseBasicParsing http://127.0.0.1:47989/serverinfo -TimeoutSec 10).Content } catch { "serverinfo: $_"; Start-Sleep -Seconds 10; continue }
            $others = @(Get-CimInstance Win32_Process -Filter "Name='butterpollo.exe'" | Where-Object { $_.ExecutablePath -notlike '*Program Files*' })
            $gpu = ((Get-Counter '\GPU Engine(*engtype_3D)\Utilization Percentage').CounterSamples | Measure-Object CookedValue -Sum).Sum
            # Nor beside a build: compiles took the CPU from the 240 fps HDR cases (October 16).
            $builds = @(Get-Process rustc, cargo, link, clippy-driver -ErrorAction SilentlyContinue).Count
            $cpu = (Get-CimInstance Win32_Processor | Measure-Object LoadPercentage -Average).Average
            if ($info.root.RustHostSessionCount -eq '0' -and $info.root.RustHostPendingSessionCount -eq '0' -and $info.root.RustHostApplicationActive -eq '0' -and $others.Count -eq 0 -and $gpu -lt 15 -and $builds -eq 0 -and $cpu -lt 20) { break }
            "waiting: sessions=$($info.root.RustHostSessionCount) app=$($info.root.RustHostApplicationActive) others=$($others.Count) gpu3d=$gpu builds=$builds cpu=$cpu"
            Start-Sleep -Seconds 30
        }
        $device = Get-PnpDevice -PresentOnly | Where-Object { $_.HardwareID -contains 'Root\RubylightVDisplay' -and $_.Status -eq 'OK' }
        if (-not $device) { "STOP: the Rubylight display driver is not running"; break }
        Remove-Item "$dir\$label.done" -ErrorAction SilentlyContinue
        $task = 'RubylightFenceRingCase'
        $command = "`"$stage\session_command.exe`" 1 C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe -NoProfile -ExecutionPolicy Bypass -File $root\run.ps1 -Label $label"
        if ($command.Length -gt 261) { throw "command too long: $($command.Length)" }
        schtasks /Create /TN $task /TR $command /SC ONCE /ST 23:59 /RU SYSTEM /RL HIGHEST /F | Out-Null
        schtasks /Run /TN $task | Out-Null
        $deadline = (Get-Date).AddSeconds($(if ($label -match '-s(\d+)$') { [int]$Matches[1] + 300 } else { 300 }))
        while (-not (Test-Path "$dir\$label.done") -and (Get-Date) -lt $deadline) { Start-Sleep -Seconds 2 }
        if (-not (Test-Path "$dir\$label.done")) { schtasks /End /TN $task | Out-Null; "${label}: timed out" }
        schtasks /Delete /TN $task /F | Out-Null
        "$label : $(Get-Content "$dir\$label.done" -ErrorAction SilentlyContinue)"
        Get-Content "$dir\$label.run.log" -Tail 1 -ErrorAction SilentlyContinue
        Start-Sleep -Seconds 1
        $after = & $snapshot
        if ($after -ne $before) { "STOP: display layout differs from before: $after"; break }
    }
} finally {
    cmd /c "schtasks /Delete /TN RubylightFenceRingCase /F >nul 2>&1"
    'cases done' | Set-Content "$dir\cases.done"
    Stop-Transcript | Out-Null
}
