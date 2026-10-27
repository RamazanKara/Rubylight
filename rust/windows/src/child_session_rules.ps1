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
#          "blocks": the enabled inbound block rules
#          "$env:BUTTERPOLLO_FIREWALL_BLOCK (TCP)" and "(UDP)", each with every
#          condition that can narrow where it applies (Get-Conditions; the host
#          requires its ports to be the RDP port and Remote Desktop over
#          WebSocket's, 3387 and 3392, and everything else to be Any),
#          "overriding": the enabled inbound allow rules that override block
#          rules (authenticated bypass), each {name: its ID, protocol,
#          local_ports}, "unprotected": profiles with the firewall off or local
#          rules ignored}
#   conditions
#          what active prints for each block rule, for every rule of the group
#          in effect; read-only (its test reads Windows' own rules with it)
param([Parameter(Mandatory)][ValidateSet('list', 'set', 'active', 'conditions')][string]$Mode)
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
# Values as one string, comma-joined; none is "".
function Join-Values($Values) { @($Values | Where-Object { $null -ne $_ } | ForEach-Object { [string]$_ }) -join ',' }
# Every condition that can narrow where $Rule applies, each as Windows names it ("Any" for none).
function Get-Conditions($Rule) {
    $ports = $Rule | Get-NetFirewallPortFilter
    $addresses = $Rule | Get-NetFirewallAddressFilter
    $application = $Rule | Get-NetFirewallApplicationFilter
    $security = $Rule | Get-NetFirewallSecurityFilter
    [ordered]@{
        name = [string]$Rule.DisplayName
        protocol = [string]$ports.Protocol
        local_ports = Join-Values $ports.LocalPort
        remote_ports = Join-Values $ports.RemotePort
        profile = [string]$Rule.Profile
        local_addresses = Join-Values $addresses.LocalAddress
        remote_addresses = Join-Values $addresses.RemoteAddress
        program = [string]$application.Program
        package = [string]$application.Package
        service = [string]($Rule | Get-NetFirewallServiceFilter).Service
        interface_types = [string]($Rule | Get-NetFirewallInterfaceTypeFilter).InterfaceType
        interfaces = Join-Values ($Rule | Get-NetFirewallInterfaceFilter).InterfaceAlias
        platforms = Join-Values $Rule.Platform
        dynamic_addresses = Join-Values $Rule.RemoteDynamicKeywordAddresses
        local_users = [string]$security.LocalUser
        remote_users = [string]$security.RemoteUser
        remote_machines = [string]$security.RemoteMachine
    }
}

if ($Mode -eq 'list') {
    $ids = @(Get-GroupRules | Where-Object { $_.Enabled -eq 'True' } | ForEach-Object { $_.Name })
    ConvertTo-Json -InputObject $ids -Compress
} elseif ($Mode -eq 'conditions') {
    $rules = @(Get-GroupRules 'ActiveStore' | ForEach-Object { Get-Conditions $_ })
    ConvertTo-Json -Compress -Depth 4 -InputObject $rules
} elseif ($Mode -eq 'active') {
    $block = $env:BUTTERPOLLO_FIREWALL_BLOCK
    if (-not $block) { throw 'no block rule was given' }
    $inbound = { $_.Enabled -eq 'True' -and $_.Direction -eq 'Inbound' }
    $allowed = @(Get-GroupRules 'ActiveStore' | Where-Object $inbound |
            Where-Object { $_.Action -eq 'Allow' } | ForEach-Object { $_.Name })
    $names = "$block (TCP)", "$block (UDP)"
    # The name is not enough: each rule's ports, protocol and every other condition.
    $blocks = @(Get-Rules @{ PolicyStore = 'ActiveStore'; DisplayName = $names } |
            Where-Object $inbound | Where-Object { $_.Action -eq 'Block' } |
            ForEach-Object { Get-Conditions $_ })
    # An allow rule that overrides block rules wins over them, whatever its group.
    $overriding = @(Get-NetFirewallSecurityFilter -PolicyStore ActiveStore -All |
            Where-Object { $_.OverrideBlockRules -eq 'True' } | Get-NetFirewallRule |
            Where-Object $inbound | Where-Object { $_.Action -eq 'Allow' } | ForEach-Object {
                $ports = $_ | Get-NetFirewallPortFilter
                [ordered]@{
                    name = [string]$_.Name; protocol = [string]$ports.Protocol
                    local_ports = Join-Values $ports.LocalPort
                }
            })
    $unprotected = @(Get-NetFirewallProfile -PolicyStore ActiveStore |
            Where-Object { $_.Enabled -ne 'True' -or $_.AllowLocalFirewallRules -eq 'False' } |
            ForEach-Object { [string]$_.Name })
    ConvertTo-Json -Compress -Depth 4 -InputObject ([ordered]@{
            allowed = $allowed; blocks = $blocks; overriding = $overriding; unprotected = $unprotected
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
