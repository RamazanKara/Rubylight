# Run elevated by the harness (host.cjs, `system: true`) through an on-demand task that is registered
# for the signed-in user with highest privileges. Does the few things that need administrator rights
# and nothing else: create, run, end and delete the one-shot SYSTEM task that starts the test host,
# stop what is left of that host, and allow it through the firewall. The request is a JSON file; every step is checked against the
# test host's own task name and staging directory before it runs.
$ErrorActionPreference = 'Continue'
$dir = 'C:\src\butterpollo-release\wr\elevate'
$taskName = 'RubylightWebrtcE2eHost'
$staging = 'C:\tests\webrtc-host'
$request = Get-Content (Join-Path $dir 'request.json') -Raw | ConvertFrom-Json
$results = @()
foreach ($step in $request.steps) {
    $a = @($step.args)
    $verdict = $null
    if ($step.kill) {
        foreach ($p in Get-CimInstance Win32_Process) {
            if ($p.ExecutablePath -and $p.ExecutablePath.StartsWith($staging + '\', [StringComparison]::OrdinalIgnoreCase)) { Stop-Process -Id $p.ProcessId -Force -ErrorAction SilentlyContinue; $verdict += "killed $($p.Name) " }
        }
        $results += [pscustomobject]@{exit = 0; output = "$verdict"}
        continue
    }
    if ($step.firewall -and $step.remove) {
        Get-NetFirewallRule -DisplayName 'Rubylight WebRTC e2e test host' -ErrorAction SilentlyContinue | Remove-NetFirewallRule
        $results += [pscustomobject]@{exit = 0; output = 'firewall rule removed'}
        continue
    }
    if ($step.firewall) {
        # An inbound allow rule, for this run only (the harness removes it when the host stops), for the staged host, so that Windows does not ask (a modal dialog
        # dims the clock window and the glass-to-glass probe cannot find it).
        $name = 'Rubylight WebRTC e2e test host'
        $program = Join-Path $staging 'butterpollo.exe'
        $rule = Get-NetFirewallRule -DisplayName $name -ErrorAction SilentlyContinue
        if ($rule -and (($rule | Get-NetFirewallApplicationFilter).Program -ne $program)) { $rule | Remove-NetFirewallRule; $rule = $null }
        if (-not $rule) { New-NetFirewallRule -DisplayName $name -Direction Inbound -Action Allow -Profile Any -Program $program | Out-Null }
        $results += [pscustomobject]@{exit = 0; output = 'firewall rule present'}
        continue
    }
    $valid = $a.Count -ge 3 -and $a[0] -in '/Create', '/Run', '/End', '/Delete' -and $a[1] -eq '/TN' -and $a[2] -eq $taskName
    if ($valid -and $a[0] -eq '/Create') {
        $at = [array]::IndexOf($a, '/TR')
        $valid = $at -gt 0 -and ([string]$a[$at + 1]).StartsWith("$staging\system_launch.exe ") -and $a -contains '/RU' -and $a[[array]::IndexOf($a, '/RU') + 1] -eq 'SYSTEM'
    }
    if (-not $valid) { $results += [pscustomobject]@{exit = 99; output = "refused: $($a -join ' ')"}; continue }
    $output = & schtasks.exe @a 2>&1 | Out-String
    $results += [pscustomobject]@{exit = $LASTEXITCODE; output = $output.Trim()}
}
[pscustomobject]@{id = $request.id; results = $results} | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $dir 'result.json') -Encoding utf8
