# Runs every ready hardware test case of one device from tools\hw\catalog.yaml
# in one batch, after the precondition checks, and collects the results into
# C:\src\butterpollo-release\hw\<date>\<device>\ with a summary.md. Exit code 3
# (an AMD encoder hang) stops the batch. The logic lives in tools\hw\run.py.
#   tools\hw\run.ps1 -Batch desktop -DryRun
#   tools\hw\run.ps1 -Batch desktop -Only e2e-hevc,soak-30min
#   tools\hw\run.ps1 -Batch desktop -Only couch-audio-71 -Var repo=C:\src\rl-audio-h2
param(
    [Parameter(Mandatory)][ValidateSet('desktop', 'laptop', 'nuc', 'retroid', 'user')] [string] $Batch,
    [string[]] $Only,
    [string[]] $Var,
    [string] $OutRoot,
    [switch] $DryRun
)
$py = @("$PSScriptRoot\run.py", '--batch', $Batch)
# `powershell -File` passes "a,b" as one string.
if ($Only) { $py += @('--only') + ($Only -split ',' | Where-Object { $_ }) }
foreach ($v in $Var) { $py += @('--var', $v) }
if ($OutRoot) { $py += @('--out-root', $OutRoot) }
if ($DryRun) { $py += '--dry-run' }
& python @py
exit $LASTEXITCODE
