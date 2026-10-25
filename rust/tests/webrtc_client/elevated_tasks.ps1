# Run elevated by the harness (host.cjs, `system: true`) through an on-demand task that is registered
# for the signed-in user with highest privileges. Does the few things that need administrator rights
# and nothing else: create, run, end and delete the one-shot SYSTEM task that starts the test host,
# and stop what is left of that host. The request is a JSON file; every step is checked against the
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
