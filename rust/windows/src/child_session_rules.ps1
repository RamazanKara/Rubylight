# Lists or switches the firewall rules of one group for the child-session
# setup (butterpollo_windows::child_session). The host runs it from stdin with
# `-NoProfile -NonInteractive -ExecutionPolicy Bypass`. Rules are named by
# their ID (Get-NetFirewallRule's Name), which is unique; display names are
# not, so a rule is never found by its display name.
#
#   list   the group in $env:BUTTERPOLLO_FIREWALL_GROUP; prints a JSON array
#          of the IDs of its enabled rules
#   set    turns the rule $env:BUTTERPOLLO_FIREWALL_RULE of that group on
#          ($env:BUTTERPOLLO_FIREWALL_ENABLED = 1) or off (0)
#   active what is in effect now (the active store, group policy included):
#          prints {"allowed": IDs of the group's enabled inbound allow rules,
#          "blocked": whether the enabled inbound block rules
#          "$env:BUTTERPOLLO_FIREWALL_BLOCK (TCP)" and "(UDP)" are both there,
#          "block_ports": the local ports of each of those rules, comma-joined,
#          "unprotected": profiles with the firewall off or local rules ignored}
param([Parameter(Mandatory)][ValidateSet('list', 'set', 'active')][string]$Mode)
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$group = $env:BUTTERPOLLO_FIREWALL_GROUP
if (-not $group) { throw 'no firewall group was given' }

function Get-Rules([hashtable]$Filter) {
    # No rule found is an ObjectNotFound error; every other error is real.
    $rules = @(Get-NetFirewallRule @Filter -ErrorAction SilentlyContinue -ErrorVariable problems)
    $real = @($problems | Where-Object { $_.CategoryInfo.Category -ne 'ObjectNotFound' })
    if ($real.Count) { throw $real[0] }
    $rules
}
function Get-GroupRules([string]$Store = 'PersistentStore') {
    Get-Rules @{ PolicyStore = $Store; Group = $group }
}

if ($Mode -eq 'list') {
    $ids = @(Get-GroupRules | Where-Object { $_.Enabled -eq 'True' } | ForEach-Object { $_.Name })
    ConvertTo-Json -InputObject $ids -Compress
} elseif ($Mode -eq 'active') {
    $block = $env:BUTTERPOLLO_FIREWALL_BLOCK
    if (-not $block) { throw 'no block rule was given' }
    $inbound = { $_.Enabled -eq 'True' -and $_.Direction -eq 'Inbound' }
    $allowed = @(Get-GroupRules 'ActiveStore' | Where-Object $inbound |
            Where-Object { $_.Action -eq 'Allow' } | ForEach-Object { $_.Name })
    $names = "$block (TCP)", "$block (UDP)"
    $rules = @(Get-Rules @{ PolicyStore = 'ActiveStore'; DisplayName = $names } |
            Where-Object $inbound | Where-Object { $_.Action -eq 'Block' })
    $blocks = @($rules | ForEach-Object { $_.DisplayName })
    $blocked = [bool](@($names | Where-Object { $blocks -ccontains $_ }).Count -eq $names.Count)
    # The name is not enough: each rule's ports, which the host compares with the RDP port.
    $ports = @($rules | ForEach-Object { @(($_ | Get-NetFirewallPortFilter).LocalPort) -join ',' })
    $unprotected = @(Get-NetFirewallProfile -PolicyStore ActiveStore |
            Where-Object { $_.Enabled -ne 'True' -or $_.AllowLocalFirewallRules -eq 'False' } |
            ForEach-Object { [string]$_.Name })
    ConvertTo-Json -Compress -InputObject ([ordered]@{
            allowed = $allowed; blocked = $blocked; block_ports = $ports; unprotected = $unprotected
        })
} else {
    $id = $env:BUTTERPOLLO_FIREWALL_RULE
    if (-not $id) { throw 'no firewall rule was given' }
    # Matched by equality inside the group, never as a -Name wildcard.
    $rule = @(Get-GroupRules | Where-Object { $_.Name -eq $id })
    if ($rule.Count -ne 1) { throw "the firewall rule $id is not in the group $group" }
    switch ($env:BUTTERPOLLO_FIREWALL_ENABLED) {
        '1' { $rule[0] | Enable-NetFirewallRule }
        '0' { $rule[0] | Disable-NetFirewallRule }
        default { throw 'the rule is switched on (1) or off (0)' }
    }
}
