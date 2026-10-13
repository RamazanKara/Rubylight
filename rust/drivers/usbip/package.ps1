# The original upstream installer, including its Microsoft-signed drivers.
# Shared by packaging and setup so neither can silently use a different build.
$UsbIpVersion = '0.9.8.1'
$UsbIpInstallerName = 'USBip-0.9.8.1-x64.exe'
$UsbIpInstallerUrl = 'https://github.com/vadimgrn/usbip-win2/releases/download/v.0.9.8.1/USBip-0.9.8.1-x64.exe'
$UsbIpInstallerHash = '38cad6d4432b52d5bb9409d9ad03b72fdffc4ada4cd3a48fbeca1a2752a8518a'
$UsbIpUninstallKey = 'SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\{199505b0-b93d-4521-a8c7-897818e0205a}_is1'

function Assert-UsbIpInstaller([string]$Path) {
    if ((Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash -ne $UsbIpInstallerHash) {
        throw 'The bundled USB/IP installer does not match the pinned upstream release.'
    }
    $signature = Get-AuthenticodeSignature -LiteralPath $Path
    if ($signature.Status -ne 'Valid' -or !$signature.SignerCertificate) {
        throw "The bundled USB/IP installer has no valid Authenticode signature: $($signature.Status)"
    }
}

# Only query state here. An existing shared installation must never be
# downgraded, uninstalled or repaired by running the nested installer over it.
function Get-UsbIpState {
    $folders = @((Join-Path $env:ProgramFiles 'USBip'))
    $registered = $false
    foreach ($key in @("HKLM:\$UsbIpUninstallKey", "HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\{199505b0-b93d-4521-a8c7-897818e0205a}_is1")) {
        if (Test-Path -LiteralPath $key) {
            $registered = $true
            $entry = Get-ItemProperty -LiteralPath $key
            if ($entry.InstallLocation) { $folders += $entry.InstallLocation }
        }
    }
    $folders += $env:PATH -split ';' | Where-Object { $_ }
    $client = $folders | ForEach-Object { Join-Path $_ 'usbip.exe' } |
        Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
    $ude = Test-Path -LiteralPath 'HKLM:\SYSTEM\CurrentControlSet\Services\usbip2_ude'
    $filter = Test-Path -LiteralPath 'HKLM:\SYSTEM\CurrentControlSet\Services\usbip2_filter'
    [pscustomobject]@{ Existing = ($registered -or $ude -or $filter -or !!$client); Ready = ($ude -and !!$client) }
}
