# Developer loop helpers. `check` runs the smallest set of checks for the
# files changed against the merge base with -Base (default origin/main) plus
# uncommitted ones, prints one PASS/FAIL table and keeps the logs in files
# (C:\src\butterpollo-release\unified\dev-check\<worktree>\). The mapping lives
# in tools/dev.py.
#   tools\dev.ps1 check -DryRun           # the plan only
#   tools\dev.ps1 check -Base HEAD        # only uncommitted changes
param(
    [Parameter(Mandatory, Position = 0)][ValidateSet('check')] [string] $Command,
    [string] $Base = 'origin/main',
    [string] $LogDir,
    [string] $TargetDir,
    [switch] $DryRun
)
$py = @("$PSScriptRoot\dev.py", $Command, '--base', $Base)
if ($LogDir) { $py += @('--log-dir', $LogDir) }
if ($TargetDir) { $py += @('--target-dir', $TargetDir) }
if ($DryRun) { $py += '--dry-run' }
& python @py
exit $LASTEXITCODE
