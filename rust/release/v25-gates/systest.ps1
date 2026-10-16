# Runs one ignored test of a butterpollo-windows test binary as SYSTEM in the signed-in session.
# Elevated. systest.ps1 -Exe <test exe> -Test <exact name> -Out <log>
param([Parameter(Mandatory)] [string] $Exe, [Parameter(Mandatory)] [string] $Test, [Parameter(Mandatory)] [string] $Out)
$root = 'C:\src\butterpollo-release\fence-ring'
$inner = "$root\systest-inner.ps1"
@"
`$env:PATH = 'C:\src\butterpollo-release\2.4.5\release\butterpollo-rust-release;' + `$env:PATH
& '$Exe' --ignored --exact '$Test' --test-threads=1 --nocapture *> '$Out'
"exit=`$LASTEXITCODE `$(whoami)" | Add-Content '$Out'
'done' | Set-Content '$Out.done'
"@ | Set-Content $inner
Remove-Item "$Out.done" -ErrorAction SilentlyContinue
$task = 'RubylightFenceRingSysTest'
$command = "`"C:\src\butterpollo-release\stage-latency\session_command.exe`" 1 C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe -NoProfile -ExecutionPolicy Bypass -File $inner"
schtasks /Create /TN $task /TR $command /SC ONCE /ST 23:59 /RU SYSTEM /RL HIGHEST /F | Out-Null
schtasks /Run /TN $task | Out-Null
$deadline = (Get-Date).AddMinutes(10)
while (-not (Test-Path "$Out.done") -and (Get-Date) -lt $deadline) { Start-Sleep -Seconds 2 }
schtasks /Delete /TN $task /F | Out-Null
Get-Content $Out | Select-String 'test result|panicked|Error|exit='
