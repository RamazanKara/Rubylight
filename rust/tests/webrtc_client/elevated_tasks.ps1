# The WebRTC end-to-end harness's administrator steps (host.cjs, `elevate`). rust/release/elevation.ps1
# installs it as C:\ProgramData\ButterpolloRelease\webrtc-e2e-elevated.ps1, where only administrators
# and SYSTEM can change it, and the ButterpolloReleaseElevated task runs it (through elevated-task.ps1)
# when the harness has written a request. An elevated harness runs the checkout's copy directly.
#
# It does the few things that need administrator rights and nothing else: create, run, end and delete
# the one-shot SYSTEM task that starts the staged test host through system_launch, stop what is left
# of that host, and allow the host through the firewall for the run. The request is a JSON file that
# anything running as the user can write, so every step is checked against the test host's own task
# name, staging directory and exact command line before it runs. -DryRun checks without running
# (host.test.cjs); -Dir points it at another request folder for that test.
#
# system_launch.exe runs from the installed folder, where the user cannot replace it. The test host
# it starts, C:\tests\webrtc-host\butterpollo.exe, and the DLLs beside it are the user's: they run as
# SYSTEM whatever they are, which is the trade-off elevation.ps1 states.
param([string] $Dir = 'C:\src\butterpollo-release\webrtc-e2e', [switch] $DryRun)
$ErrorActionPreference = 'Continue'
$taskName = 'RubylightWebrtcE2eHost'
$staging = 'C:\tests\webrtc-host'
$launcher = 'C:\ProgramData\ButterpolloRelease\system_launch.exe'
$rule = 'Rubylight WebRTC e2e test host'
$requestFile = Join-Path $Dir 'request.json'
$request = Get-Content $requestFile -Raw | ConvertFrom-Json
# Spent once read: a request is never run twice.
Remove-Item $requestFile -Force
# The SYSTEM task starts system_launch, which starts the staged host with plain arguments: no quotes,
# no shell characters, no other program.
$command = '^' + [regex]::Escape("$launcher $staging\butterpollo.exe") + '( [^\s"''&|<>^%]+)*$'
$results = @()
foreach ($step in $request.steps) {
    $a = @($step.args | ForEach-Object { [string]$_ })
    if ($step.kill) {
        $verdict = ''
        foreach ($p in Get-CimInstance Win32_Process) {
            if ($p.ExecutablePath -and $p.ExecutablePath.StartsWith($staging + '\', [StringComparison]::OrdinalIgnoreCase)) {
                if (-not $DryRun) { Stop-Process -Id $p.ProcessId -Force -ErrorAction SilentlyContinue }
                $verdict += "killed $($p.Name) "
            }
        }
        $results += [pscustomobject]@{exit = 0; output = $verdict.Trim()}
        continue
    }
    if ($step.firewall) {
        # Inbound for the staged host only, from the local network, while the run lasts (the harness
        # removes it when the host stops, the next run removes one a killed run left), so that Windows
        # does not ask: a modal dialog dims the clock window and the glass-to-glass probe cannot find it.
        # A rule of that name is always replaced, never trusted.
        $existing = @(Get-NetFirewallRule -DisplayName $rule -ErrorAction SilentlyContinue)
        if (-not $DryRun) { $existing | Remove-NetFirewallRule -ErrorAction SilentlyContinue }
        $output = if ($existing.Count) { 'firewall rule removed' } else { 'no firewall rule' }
        if (-not $step.remove) {
            if (-not $DryRun) {
                New-NetFirewallRule -DisplayName $rule -Direction Inbound -Action Allow -Profile Private `
                    -RemoteAddress LocalSubnet -Program (Join-Path $staging 'butterpollo.exe') | Out-Null
            }
            $output = 'firewall rule created'
        }
        $results += [pscustomobject]@{exit = 0; output = $output}
        continue
    }
    $valid = switch ($a[0]) {
        '/Create' {
            $a.Count -eq 14 -and $a[1] -eq '/TN' -and $a[2] -eq $taskName -and $a[3] -eq '/TR' -and $a[4] -cmatch $command -and
                (($a[5..13] -join ' ') -ceq '/SC ONCE /ST 23:59 /RU SYSTEM /RL HIGHEST /F')
        }
        { $_ -in '/Run', '/End' } { $a.Count -eq 3 -and $a[1] -eq '/TN' -and $a[2] -eq $taskName }
        '/Delete' { $a.Count -eq 4 -and $a[1] -eq '/TN' -and $a[2] -eq $taskName -and $a[3] -eq '/F' }
        default { $false }
    }
    if (-not $valid) { $results += [pscustomobject]@{exit = 99; output = "refused: $($a -join ' ')"}; continue }
    if ($DryRun) { $results += [pscustomobject]@{exit = 0; output = "would run: $($a -join ' ')"}; continue }
    $output = & schtasks.exe @a 2>&1 | Out-String
    $results += [pscustomobject]@{exit = $LASTEXITCODE; output = $output.Trim()}
}
[pscustomobject]@{id = $request.id; results = @($results)} | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $Dir 'result.json') -Encoding utf8
