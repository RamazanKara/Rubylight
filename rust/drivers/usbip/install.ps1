# Install the optional shared Steam Deck / DualSense transport, offline.
param([switch]$ValidateOnly)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'package.ps1')
. (Join-Path $PSScriptRoot 'actions.ps1')

try {
    $installer = Join-Path $PSScriptRoot $UsbIpInstallerName
    Assert-UsbIpInstaller $installer
    if ($ValidateOnly) {
        Write-Output 'Bundled USB/IP installer hash and signature verified; nothing installed.'
        exit 0
    }
    if (![Environment]::Is64BitProcess -or $env:PROCESSOR_ARCHITECTURE -ne 'AMD64' -or
        [Environment]::OSVersion.Version.Build -lt 18362) {
        throw 'USB audio and haptics requires Windows 10 x64 version 1903 or later.'
    }
    $principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
    if (!$principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'Administrator privileges are required to install USB audio and haptics.'
    }
    $code = Install-UsbIpTransport $installer (Join-Path $env:TEMP 'Rubylight-usbip-install.log')
    exit $code
} catch {
    Write-Output "USBIP_DRIVER_WARNING: $($_.Exception.Message)"
    exit 1
}
