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
param([Parameter(Mandatory)][ValidateSet('list', 'set')][string]$Mode)
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$group = $env:BUTTERPOLLO_FIREWALL_GROUP
if (-not $group) { throw 'no firewall group was given' }

function Get-GroupRules {
    # A group without rules is an ObjectNotFound error; every other error is real.
    $rules = @(Get-NetFirewallRule -PolicyStore PersistentStore -Group $group `
            -ErrorAction SilentlyContinue -ErrorVariable problems)
    $real = @($problems | Where-Object { $_.CategoryInfo.Category -ne 'ObjectNotFound' })
    if ($real.Count) { throw $real[0] }
    $rules
}

if ($Mode -eq 'list') {
    $ids = @(Get-GroupRules | Where-Object { $_.Enabled -eq 'True' } | ForEach-Object { $_.Name })
    ConvertTo-Json -InputObject $ids -Compress
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
