<#
.SYNOPSIS
One-time setup so check.ps1 no longer needs a UAC prompt.

.DESCRIPTION
Run once from an elevated PowerShell. It copies elevated.ps1 and
elevated-task.ps1 to C:\ProgramData\ButterpolloRelease, where only
administrators and SYSTEM can change them, and registers the on-demand task
"ButterpolloReleaseElevated" that runs them for you with highest privileges.
check.ps1 starts that task instead of asking for elevation, as long as the
installed scripts match the ones in the checkout; after they change, it asks
once more and tells you to run this again.

The same task serves the WebRTC end-to-end harness: webrtc-e2e-elevated.ps1
(from rust/tests/webrtc_client/elevated_tasks.ps1) starts its staged test host
as SYSTEM through system_launch.exe, which is installed here as well, and opens
the firewall for it while a run lasts.

Trade-off: any program running as you can start the task, and the task runs
the release's setup and the display self-test from your release work folder
with administrator rights. The WebRTC test host, C:\tests\webrtc-host\butterpollo.exe,
and the DLLs beside it run as SYSTEM, and that folder stays writable by you:
anything running as you can replace them before a harness run. Only install
it on a machine where you accept that.

.PARAMETER SystemLaunch
The built system_launch example (cargo build --release -p butterpollo-windows
--example system_launch) to install. Without it, the one from the harness's
default build is installed if it is there; otherwise SYSTEM test hosts refuse
to start until this runs again with it.

.EXAMPLE
pwsh -File rust/release/elevation.ps1           # install or update
.EXAMPLE
pwsh -File rust/release/elevation.ps1 -Remove   # uninstall
#>
#Requires -RunAsAdministrator
param(
    [switch] $Remove,
    [string] $SystemLaunch
)
$ErrorActionPreference = 'Stop'
$task = 'ButterpolloReleaseElevated'
$home_ = 'C:\ProgramData\ButterpolloRelease'

Unregister-ScheduledTask -TaskName $task -Confirm:$false -ErrorAction SilentlyContinue
if ($Remove) {
    Remove-Item -Recurse -Force $home_ -ErrorAction SilentlyContinue
    "Removed $task and $home_."
    return
}

New-Item -ItemType Directory -Force $home_ | Out-Null
# Administrators and SYSTEM may change the scripts; everyone else may only run them.
icacls $home_ /inheritance:r /grant:r '*S-1-5-32-544:(OI)(CI)F' '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-545:(OI)(CI)RX' | Out-Null
foreach ($script in 'elevated.ps1', 'elevated-task.ps1') {
    Copy-Item (Join-Path $PSScriptRoot $script) $home_ -Force
}
# The WebRTC end-to-end harness's steps (rust/tests/webrtc_client/host.cjs), served by the same task.
Copy-Item (Join-Path $PSScriptRoot '..\tests\webrtc_client\elevated_tasks.ps1') (Join-Path $home_ 'webrtc-e2e-elevated.ps1') -Force
# Its SYSTEM task runs this launcher from here, so the program that checks what it starts cannot be
# replaced by the user (the staged host it starts can be: see the trade-off above).
$launcher = if ($SystemLaunch) { $SystemLaunch } else { 'C:\src\butterpollo-release\wr\target\release\examples\system_launch.exe' }
if (Test-Path -LiteralPath $launcher -PathType Leaf) {
    Copy-Item -LiteralPath $launcher (Join-Path $home_ 'system_launch.exe') -Force
} elseif ($SystemLaunch) {
    throw "no system_launch.exe at $SystemLaunch"
} else {
    Write-Warning "No system_launch.exe at $launcher, so the WebRTC harness cannot start a SYSTEM test host. Build it (cargo build --release -p butterpollo-windows --example system_launch) and run this again with -SystemLaunch <path>."
}
# On demand only (no trigger), for the signed-in user, with the elevated token.
# An interactive principal needs no stored password.
$action = New-ScheduledTaskAction -Execute 'powershell.exe' `
    -Argument "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$home_\elevated-task.ps1`""
$principal = New-ScheduledTaskPrincipal -UserId "$env:USERDOMAIN\$env:USERNAME" -LogonType Interactive -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
    -ExecutionTimeLimit (New-TimeSpan -Minutes 30) -MultipleInstances IgnoreNew
Register-ScheduledTask -TaskName $task -Action $action -Principal $principal -Settings $settings `
    -Description 'Rubylight release: display self-test and install (rust/release/elevation.ps1)' | Out-Null
"Installed $task for $env:USERDOMAIN\$env:USERNAME. check.ps1 now installs and self-tests without a UAC prompt."
