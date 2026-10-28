# Turns one firewall rule of a group back on, for the restore of the removed
# child-session setup (butterpollo_windows::child_session). The host runs it
# from stdin with `-NoProfile -NonInteractive -ExecutionPolicy Bypass`. Rules
# are named by their ID (Get-NetFirewallRule's Name), which is unique; display
# names are not, so a rule is never found by its display name.
#
#   enable turns the rule $env:BUTTERPOLLO_FIREWALL_RULE of the group in
#          $env:BUTTERPOLLO_FIREWALL_GROUP on
param([Parameter(Mandatory)][ValidateSet('enable')][string]$Mode)
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$group = $env:BUTTERPOLLO_FIREWALL_GROUP
if (-not $group) { throw 'no firewall group was given' }
$id = $env:BUTTERPOLLO_FIREWALL_RULE
if (-not $id) { throw 'no firewall rule was given' }

# No rule found is an ObjectNotFound error; every other error is real.
$rules = @(Get-NetFirewallRule -PolicyStore PersistentStore -Group $group -ErrorAction SilentlyContinue -ErrorVariable problems)
$real = @($problems | Where-Object { $_.CategoryInfo.Category -ne 'ObjectNotFound' })
if ($real.Count) { throw $real[0] }
# Matched by equality inside the group, never as a -Name wildcard.
$rule = @($rules | Where-Object { $_.Name -eq $id })
if ($rule.Count -ne 1) { throw "the firewall rule $id is not in the group $group" }
$rule[0] | Enable-NetFirewallRule
