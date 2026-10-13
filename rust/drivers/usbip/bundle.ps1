# Package only: this never launches the USB/IP installer or changes drivers.
param([Parameter(Mandatory)][string]$Cache, [Parameter(Mandatory)][string]$Destination)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'package.ps1')
New-Item -ItemType Directory -Path $Cache, $Destination -Force | Out-Null
$installer = Join-Path $Cache $UsbIpInstallerName
if (!(Test-Path -LiteralPath $installer)) {
    Invoke-WebRequest -Uri $UsbIpInstallerUrl -OutFile $installer -TimeoutSec 180
}
Assert-UsbIpInstaller $installer
Copy-Item -LiteralPath $installer -Destination $Destination -Force
foreach ($name in @('install.ps1', 'package.ps1', 'actions.ps1', 'LICENSE.txt')) {
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot $name) -Destination $Destination -Force
}
Write-Output "Bundled usbip-win2 $UsbIpVersion for Steam Deck and DualSense USB audio and haptics."
