# Draws motion_probe on every display that appears while this runs, for 80 s per appearance:
# the laptop's virtual display during rust/release/smoke.ps1, which needs motion at twice the
# stream's rate for the whole session (the host sends a frame only when the picture changes).
#   pwsh tools/hw/probe-on-new-display.ps1 -Minutes 5 -Out <dir>
param([int]$Minutes = 5, [Parameter(Mandatory)][string]$Out, [int]$Hz = 240)
$probe = 'C:\src\butterpollo-release\fixtures\motion_probe.exe'
New-Item -ItemType Directory -Force $Out | Out-Null
# A fresh process per poll: a running one keeps its first list of screens.
$list = { @(powershell -NoProfile -Command "Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.Screen]::AllScreens | ForEach-Object { `$_.DeviceName + '|' + `$_.Bounds.Width + 'x' + `$_.Bounds.Height }") }
$known = @(& $list | ForEach-Object { ($_ -split '\|')[0] })
"{0:HH:mm:ss} displays at start: {1}" -f (Get-Date), ($known -join ', ')
$end = (Get-Date).AddMinutes($Minutes)
$running = @{}
while ((Get-Date) -lt $end) {
    $now = @(& $list)
    $names = @($now | ForEach-Object { ($_ -split '\|')[0] })
    foreach ($entry in $now) {
        $name = ($entry -split '\|')[0]
        if ($known -notcontains $name -and -not ($running[$name] -and -not $running[$name].HasExited)) {
            $json = Join-Path $Out ("probe-{0:HHmmss}.json" -f (Get-Date))
            $running[$name] = Start-Process $probe -ArgumentList $name, 80, $json, $Hz -PassThru -WindowStyle Hidden
            "{0:HH:mm:ss} probe on {1}" -f (Get-Date), $entry
        }
    }
    foreach ($name in @($running.Keys)) {
        if ($names -notcontains $name -and -not $running[$name].HasExited) {
            Stop-Process -Id $running[$name].Id -Force -ErrorAction SilentlyContinue
            "{0:HH:mm:ss} {1} is gone, probe stopped" -f (Get-Date), $name
        }
    }
    Start-Sleep -Milliseconds 400
}
foreach ($p in $running.Values) { if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue } }
"{0:HH:mm:ss} done" -f (Get-Date)
