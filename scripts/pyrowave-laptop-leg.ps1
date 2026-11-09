# One streaming leg of scripts/pyrowave-laptop.ps1 on the Windows laptop: runs nightjar from
# <Dir>\app in the console user's desktop session (a scheduled task, as ssh has no desktop), with
# <Dir>\cfg as its settings folder, streaming from <Address> for <Seconds>, and leaves the timing
# CSV and the client's output in <Dir>\out.
#
#   pyrowave-laptop-leg.ps1 -Dir C:\tests\njac -Name 01-A-133 -Seconds 60 -Address 192.168.4.69:48463
param(
    [Parameter(Mandatory)][string]$Dir,
    [Parameter(Mandatory)][string]$Name,
    [int]$Seconds = 60,
    [Parameter(Mandatory)][string]$Address,
    [string]$Pin = '1234'
)
$ErrorActionPreference = 'Stop'
$out = Join-Path $Dir 'out'
New-Item -ItemType Directory -Force $out | Out-Null
$csv = Join-Path $out "$Name.csv"
$log = Join-Path $out "$Name.log"
$done = Join-Path $out "$Name.done"
Remove-Item $csv, $log, $done -ErrorAction SilentlyContinue
@"
@echo off
set NIGHTJAR_CONFIG_DIR=$Dir\cfg
set NIGHTJAR_AUTOSTREAM=$Address
set NIGHTJAR_AUTOSTREAM_APP=Desktop
set NIGHTJAR_AUTOSTREAM_PIN=$Pin
set NIGHTJAR_AUTOSTREAM_SECONDS=$Seconds
set NIGHTJAR_TIMING_CSV=$csv
set NIGHTJAR_EARLY_DECODE=0
set NO_PROXY=*
cd /d $Dir\app
nightjar.exe > $log 2>&1
echo %ERRORLEVEL% > $done
"@ | Set-Content -Encoding ascii "$Dir\task.cmd"
$console = (Get-CimInstance Win32_ComputerSystem).UserName
if (-not $console) { throw 'nobody is signed in at the console' }
$action = New-ScheduledTaskAction -Execute "$env:WINDIR\System32\cmd.exe" -Argument "/c $Dir\task.cmd"
$principal = New-ScheduledTaskPrincipal -UserId $console -LogonType Interactive
Register-ScheduledTask -TaskName njac -Action $action -Principal $principal -Force | Out-Null
try {
    Start-ScheduledTask -TaskName njac
    $deadline = (Get-Date).AddSeconds($Seconds + 120)
    while (-not (Test-Path $done)) {
        if ((Get-Date) -gt $deadline) { Get-Process nightjar -ErrorAction SilentlyContinue | Stop-Process -Force; throw "$Name did not finish" }
        Start-Sleep -Seconds 2
    }
} finally {
    Unregister-ScheduledTask -TaskName njac -Confirm:$false -ErrorAction SilentlyContinue
}
$code = [int](Get-Content $done -Raw).Trim()
Get-Content $log | Select-String 'PyroWave decodes on|autostream|GPU decod' | ForEach-Object { $_.Line }
"$Name exit $code, csv rows: $((Get-Content $csv -ErrorAction SilentlyContinue | Measure-Object -Line).Lines)"
exit $code
