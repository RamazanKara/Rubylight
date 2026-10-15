# Removes the libvirtualdisplay (Sunshine/Vibepollo) virtual display driver that Rubylight
# installed before it moved to its own driver: the Root\SunshineVirtualDisplay device, its
# driver packages and its Vulkan HDR layer registration. Setup runs it as SYSTEM on every
# install and update; with nothing left it changes nothing. Elevated.
#
# The SignPath Foundation certificate the old script added to TrustedPublisher stays: the
# virtual gamepad driver is signed with it too.
#
# -InstallerBestEffort reports a failure as a DRIVER_WARNING line and exits 0, as the driver
# scripts do for setup.
param([switch] $InstallerBestEffort)
$ErrorActionPreference = 'Stop'
$hardwareId = 'Root\SunshineVirtualDisplay'
$script:restart = $false

function Invoke-Pnputil([string[]] $arguments) {
    # 3010: done once Windows restarts (a display still in use).
    & pnputil @arguments | Out-Host
    if ($LASTEXITCODE -notin 0, 3010) { throw "pnputil $($arguments -join ' ') failed with exit code $LASTEXITCODE" }
    if ($LASTEXITCODE -eq 3010) { $script:restart = $true }
}

function Remove-Libvirtualdisplay {
    foreach ($device in @(Get-PnpDevice -ErrorAction SilentlyContinue | Where-Object { $_.HardwareID -contains $hardwareId })) {
        Write-Host "[RubylightSetup] Removing the libvirtualdisplay device $($device.InstanceId)."
        Invoke-Pnputil @('/remove-device', $device.InstanceId)
    }
    # pnputil's output is localized; the INF cache names the original catalog.
    $packages = Get-ChildItem "$env:SystemRoot\INF\oem*.inf" | Where-Object {
        Select-String -LiteralPath $_.FullName -Pattern '^\s*CatalogFile\s*=\s*SunshineVirtualDisplayDriver\.cat' -Quiet
    } | ForEach-Object Name
    foreach ($inf in $packages) {
        Write-Host "[RubylightSetup] Removing the libvirtualdisplay driver package $inf."
        Invoke-Pnputil @('/delete-driver', $inf, '/uninstall', '/force')
    }
    # Its HDR layer, both registry views; Rubylight registers its own.
    foreach ($view in [Microsoft.Win32.RegistryView]::Registry64, [Microsoft.Win32.RegistryView]::Registry32) {
        $root = [Microsoft.Win32.RegistryKey]::OpenBaseKey([Microsoft.Win32.RegistryHive]::LocalMachine, $view)
        try {
            $key = $root.OpenSubKey('SOFTWARE\Khronos\Vulkan\ImplicitLayers', $true)
            if (-not $key) { continue }
            try {
                foreach ($name in @($key.GetValueNames())) {
                    if ([IO.Path]::GetFileName($name) -eq 'VkLayer_sunshine_hdr.json') { $key.DeleteValue($name, $false) }
                }
            } finally { $key.Dispose() }
        } finally { $root.Dispose() }
    }
}

try {
    Remove-Libvirtualdisplay
} catch {
    if (-not $InstallerBestEffort) { throw }
    $reason = $_.Exception
    while ($reason.InnerException) { $reason = $reason.InnerException }
    Write-Warning '[RubylightSetup] DRIVER_WARNING: The old libvirtualdisplay driver was not removed completely.'
    Write-Warning "[RubylightSetup] Installer best-effort driver action failed: $($reason.Message -replace '\s+', ' ')"
}
if ($script:restart) { Write-Output 'RESTART_REQUIRED' }
