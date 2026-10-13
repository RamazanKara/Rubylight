# No elevation, downloads or real driver/installer processes.
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'actions.ps1')
function Assert($Condition, [string]$Message) { if (!$Condition) { throw $Message } }
function Get-UsbIpState {
    if ($script:Calls -gt 0) { return [pscustomobject]@{ Existing = $true; Ready = $script:ReadyAfter } }
    [pscustomobject]@{ Existing = $script:Existing; Ready = $script:Ready }
}
function Start-Process($FilePath, $ArgumentList, $WindowStyle, [switch]$Wait, [switch]$PassThru) {
    $script:Calls++
    Assert ($FilePath -eq 'C:\test only\USBip.exe') 'Wrong installer path'
    Assert ($WindowStyle -eq 'Hidden' -and $Wait -and $PassThru) 'Installer must be hidden and awaited'
    foreach ($argument in @('/NORESTART', '/RESTARTEXITCODE=3010', '/VERYSILENT', '/SUPPRESSMSGBOXES', '/COMPONENTS="main,client"', '/TASKS="vcredist"', '/LOG="C:\test only\setup.log"')) {
        Assert ($ArgumentList -contains $argument) "Missing installer argument: $argument"
    }
    [pscustomobject]@{ ExitCode = $script:ExitCode }
}
foreach ($case in @(
    @{ Name = 'existing'; Existing = $true; Ready = $true; After = $true; Code = 0; Calls = 0; Error = $false },
    @{ Name = 'incomplete shared'; Existing = $true; Ready = $false; After = $true; Code = 0; Calls = 0; Error = $true },
    @{ Name = 'fresh'; Existing = $false; Ready = $false; After = $true; Code = 0; Calls = 1; Error = $false },
    @{ Name = 'restart'; Existing = $false; Ready = $false; After = $true; Code = 3010; Calls = 1; Error = $false },
    @{ Name = 'failed installer'; Existing = $false; Ready = $false; After = $false; Code = 5; Calls = 1; Error = $true },
    @{ Name = 'missing driver after success'; Existing = $false; Ready = $false; After = $false; Code = 0; Calls = 1; Error = $true }
)) {
    $script:Calls = 0
    $script:Existing = $case.Existing
    $script:Ready = $case.Ready
    $script:ReadyAfter = $case.After
    $script:ExitCode = $case.Code
    $failed = $false
    try { $code = Install-UsbIpTransport 'C:\test only\USBip.exe' 'C:\test only\setup.log' } catch { $failed = $true }
    Assert ($failed -eq $case.Error) "Unexpected result: $($case.Name)"
    Assert ($script:Calls -eq $case.Calls) "Unexpected installer launch: $($case.Name)"
    if (!$failed) { Assert ($code -eq $case.Code) "Wrong exit code: $($case.Name)" }
}
Write-Output 'Six USB/IP installer scenarios passed without running an installer.'
