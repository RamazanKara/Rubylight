# Reads or applies the wake settings of the physical network adapters
# (docs/wake-and-audio-design.md, A.3). The host runs it from stdin with
# `-NoProfile -NonInteractive -ExecutionPolicy Bypass`. Its JSON is keyed by
# InterfaceGuid, PnP id and registry keyword, never by display names, which
# are localized; core parses and judges it (butterpollo_core::wake).
#
#   read             {"adapters": [{"guid", "pnp", "name", "wake_enable",
#                     "magic_packet_only", "keywords": {keyword: value},
#                     "errors": [...]}]}
#   apply            the FixPlan JSON in $env:BUTTERPOLLO_WAKE_PLAN; prints
#                     {"restarted": [guid], "after_reboot": [guid],
#                     "errors": [...]}
param([Parameter(Mandatory)][ValidateSet('read', 'apply')][string]$Mode)
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$keywords = @('*WakeOnMagicPacket', '*WakeOnPattern', '*ModernStandbyWoLMagicPacket', 'S5WakeOnLan')

function Find-Wmi([string]$Class, [string]$Pnp) {
    # An empty id would match every adapter's instance.
    if (-not $Pnp) { throw 'the adapter has no PnP id' }
    # InstanceName is the PnP id followed by `_0`; a longer id that starts
    # with this one is another device.
    $pattern = [Management.Automation.WildcardPattern]::Escape($Pnp) + '_*'
    @(Get-CimInstance -Namespace root\wmi -ClassName $Class |
        Where-Object { $_.InstanceName -like $pattern })
}

function Restart-Adapter($adapter) {
    # Restart-NetAdapter disables, then enables; an error between the two
    # would leave the adapter off, so it is enabled again whatever happens.
    # A killed process runs no finally: the host never kills this script
    # while it may still start a restart (see Invoke-Apply).
    try {
        $adapter | Disable-NetAdapter -Confirm:$false
    } finally {
        try {
            $adapter | Enable-NetAdapter -Confirm:$false
        } catch {
            Start-Sleep -Seconds 2
            $adapter | Enable-NetAdapter -Confirm:$false
        }
    }
}

function Read-Adapter($adapter) {
    $errors = @()
    $values = @{}
    try {
        foreach ($property in @($adapter | Get-NetAdapterAdvancedProperty -AllProperties)) {
            if ($keywords -contains $property.RegistryKeyword) {
                $values[$property.RegistryKeyword] = [string]@($property.RegistryValue)[0]
            }
        }
    } catch { $errors += "advanced properties: $_" }
    $wake = $null
    $only = $null
    try {
        $found = Find-Wmi 'MSPower_DeviceWakeEnable' $adapter.PnPDeviceID
        if ($found.Count -gt 0) { $wake = [bool]$found[0].Enable }
    } catch { $errors += "MSPower_DeviceWakeEnable: $_" }
    try {
        $found = Find-Wmi 'MSNdis_DeviceWakeOnMagicPacketOnly' $adapter.PnPDeviceID
        if ($found.Count -gt 0) { $only = [bool]$found[0].EnableWakeOnMagicPacketOnly }
    } catch { $errors += "MSNdis_DeviceWakeOnMagicPacketOnly: $_" }
    @{
        guid = ([string]$adapter.InterfaceGuid).ToUpperInvariant()
        pnp = [string]$adapter.PnPDeviceID
        name = [string]$adapter.Name
        wake_enable = $wake
        magic_packet_only = $only
        keywords = $values
        errors = $errors
    }
}

function Invoke-Apply {
    # The host kills this script at its deadline. A restart starts only while
    # $env:BUTTERPOLLO_WAKE_RESTART_BEFORE seconds have not passed, which
    # leaves a minute before the kill; after that the changed properties are
    # saved but take effect when the PC restarts.
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $restartBefore = [double]$env:BUTTERPOLLO_WAKE_RESTART_BEFORE
    if ($restartBefore -le 0) { throw 'BUTTERPOLLO_WAKE_RESTART_BEFORE is not set' }
    $plan = $env:BUTTERPOLLO_WAKE_PLAN | ConvertFrom-Json
    $errors = @()
    $restarted = @()
    $afterReboot = @()
    foreach ($fix in @($plan.adapters)) {
        $adapter = Get-NetAdapter -Physical | Where-Object { ([string]$_.InterfaceGuid) -eq $fix.guid }
        if (-not $adapter) {
            $errors += "$($fix.guid): no such adapter"
            continue
        }
        $changed = $false
        foreach ($keyword in @($fix.keywords.PSObject.Properties)) {
            try {
                $adapter | Set-NetAdapterAdvancedProperty -RegistryKeyword $keyword.Name -RegistryValue $keyword.Value -NoRestart
                $changed = $true
            } catch { $errors += "$($fix.guid) $($keyword.Name): $_" }
        }
        if ($fix.wake_enable) {
            try {
                foreach ($i in Find-Wmi 'MSPower_DeviceWakeEnable' $adapter.PnPDeviceID) {
                    Set-CimInstance -InputObject $i -Property @{ Enable = $true }
                }
            } catch { $errors += "$($fix.guid) MSPower_DeviceWakeEnable: $_" }
        }
        if ($fix.magic_packet_only) {
            try {
                foreach ($i in Find-Wmi 'MSNdis_DeviceWakeOnMagicPacketOnly' $adapter.PnPDeviceID) {
                    Set-CimInstance -InputObject $i -Property @{ EnableWakeOnMagicPacketOnly = $true }
                }
            } catch { $errors += "$($fix.guid) MSNdis_DeviceWakeOnMagicPacketOnly: $_" }
        }
        if ($changed -and $clock.Elapsed.TotalSeconds -ge $restartBefore) {
            $afterReboot += $fix.guid
        } elseif ($changed) {
            try {
                Restart-Adapter $adapter
                $restarted += $fix.guid
            } catch { $errors += "$($fix.guid) restart: $_" }
        }
    }
    if ($plan.fast_startup_off) {
        try {
            Set-ItemProperty -Path 'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager\Power' -Name HiberbootEnabled -Value 0 -Type DWord
        } catch { $errors += "HiberbootEnabled: $_" }
    }
    @{ restarted = $restarted; after_reboot = $afterReboot; errors = $errors }
}

if ($Mode -eq 'read') {
    $result = @{ adapters = @(Get-NetAdapter -Physical | ForEach-Object { Read-Adapter $_ }) }
} else {
    $result = Invoke-Apply
}
ConvertTo-Json -InputObject $result -Depth 6 -Compress
