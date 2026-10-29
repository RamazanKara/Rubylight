#Requires -Version 7
<#
.SYNOPSIS
Run isolated stream, reconnect and fault checks without changing the service.
.EXAMPLE
pwsh rust/release/soak.ps1 -Package 'C:\Program Files\ButterpolloRust' -Client target/soak-fixtures/moonlight-client.exe -GpuLoad target/qa/release/examples/gpu_load.exe
.EXAMPLE
pwsh rust/release/soak.ps1 -Mode long -Package target/soak-package -Client target/soak-fixtures/moonlight-client.exe -GpuLoad target/qa/release/examples/gpu_load.exe
#>
param(
    [ValidateSet('short', 'long')][string] $Mode = 'short',
    [Parameter(Mandatory)][string] $Package,
    [Parameter(Mandatory)][string] $Client,
    [Parameter(Mandatory)][string] $GpuLoad,
    [string] $Work = (Join-Path $PSScriptRoot '../../target/soak'),
    # Comma-separated case names to run alone, e.g. 'hevc-60-load,hevc-120-load'.
    [string] $Only = '',
    # Host settings for an A/B arm, e.g. 'amd_ltr_frames=0'.
    [string[]] $Config = @()
)
$ErrorActionPreference = 'Stop'
$python = if ($env:BUTTERPOLLO_TEST_PYTHON) { $env:BUTTERPOLLO_TEST_PYTHON } else { 'python' }
& $python "$PSScriptRoot/soak.py" --mode $Mode --package $Package --client $Client --gpu-load $GpuLoad --work $Work @(if ($Only) { '--only', $Only }) @($Config | ForEach-Object { '--config', $_ })
exit $LASTEXITCODE
