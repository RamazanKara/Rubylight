# No elevation, downloads or real driver/installer processes.
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'package.ps1')
. (Join-Path $PSScriptRoot 'actions.ps1')
function Assert($Condition, [string]$Message) { if (!$Condition) { throw $Message } }

# The search for usbip.exe has to survive whatever PATH holds: one entry with
# a stray quote ended the whole driver step (#80). The real Get-UsbIpState runs
# here on a PATH of this test's making, with the registry answering "not
# installed" and an empty Program Files, so only the PATH walk can find the
# client. This file stays ASCII: Windows PowerShell 5.1 reads it in the ANSI
# code page.
$readState = ${function:Get-UsbIpState}
function Test-Path([string]$LiteralPath, [string]$PathType) {
    if ($LiteralPath -like 'HKLM:*') { return $false }
    Microsoft.PowerShell.Management\Test-Path @PSBoundParameters
}
$root = Join-Path ([IO.Path]::GetTempPath()) ('rubylight usbip test ' + [guid]::NewGuid().ToString('N'))
$tools = Join-Path $root ("{0}a{1}r{2} M{3}ller's (x86) & co; usb" -f [char]0xC7, [char]0x11F, [char]0x131, [char]0xFC)
$unused = [char[]](71..90) | Where-Object { ![IO.Directory]::Exists("${_}:\") } | Select-Object -First 1
$stray = 'C:\Users\First Last\AppData\Local\Programs\Tool\bin"'
$broken = @('"C:\Program Files (x86)\Tool\bin"', 'C:\Tools|x', 'C:\Tools<x>', 'C:\To*ols?', "${unused}:\Tools",
    ' ', "C:\Tools`tx", 'usbip:\Tools', '\\?\C:\Tools', ('C:\' + 'x' * 300))
$saved = @{ Path = $env:PATH; ProgramFiles = $env:ProgramFiles }
try {
    [void][IO.Directory]::CreateDirectory($tools)
    [void][IO.Directory]::CreateDirectory((Join-Path $root 'Program Files'))
    [IO.File]::WriteAllBytes((Join-Path $tools 'usbip.exe'), [byte[]]@())
    $env:ProgramFiles = Join-Path $root 'Program Files'
    foreach ($case in @(
        @{ Path = "$stray;$env:SystemRoot"; Found = $false },
        @{ Path = ($broken -join ';'); Found = $false },
        @{ Path = ''; Found = $false },
        # A stray quote opens a quoted run to the end of PATH, as it does for the host.
        @{ Path = "$stray;`"$tools`""; Found = $false },
        @{ Path = (($broken + "`"$tools`"" + $stray) -join ';'); Found = $true },
        @{ Path = "`"$tools\`";$stray"; Found = $true }
    )) {
        $env:PATH = $case.Path
        try { $state = & $readState } catch { throw "A PATH entry ended the USB/IP check ($($_.Exception.Message)): $($case.Path)" }
        Assert ($state.Existing -eq $case.Found -and !$state.Ready) "Wrong USB/IP state for PATH: $($case.Path)"
    }
    $env:PATH = $saved.Path
    $entries = @(Split-UsbIpSearchPath "C:\one;;`"C:\two;three`"\four; ;$stray")
    Assert (($entries -join '|') -eq "C:\one|`"C:\two;three`"\four| |$stray") "Wrong PATH entries: $($entries -join '|')"
    Assert (@(Split-UsbIpSearchPath $null).Count -eq 0) 'An empty PATH has no entries'
    Assert ((Find-UsbIpClient ($broken + $stray + $null + '' + $tools)) -eq (Join-Path $tools 'usbip.exe')) 'The client after broken folders was not found'
    Assert ($null -eq (Find-UsbIpClient ($broken + $stray))) 'Broken folders hold no client'
    Assert ($null -eq (Find-UsbIpClient @())) 'No folders hold no client'
} finally {
    $env:PATH = $saved.Path
    $env:ProgramFiles = $saved.ProgramFiles
    Remove-Item -LiteralPath function:Test-Path
    if ([IO.Directory]::Exists($root)) { [IO.Directory]::Delete($root, $true) }
}

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
Write-Output "Six USB/IP installer scenarios and the PATH search passed without running an installer (PowerShell $($PSVersionTable.PSVersion))."

# Setup runs these scripts in Windows PowerShell 5.1, whose path cmdlets end a
# script on entries PowerShell 7 answers false for, so run there as well.
if ($PSVersionTable.PSEdition -ne 'Desktop') {
    $desktop = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
    # PowerShell 7's module path would hide Windows PowerShell's own modules.
    $modules = $env:PSModulePath
    $env:PSModulePath = $null
    try { & $desktop -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $PSCommandPath }
    finally { $env:PSModulePath = $modules }
    if ($LASTEXITCODE -ne 0) { throw "The USB/IP tests failed in Windows PowerShell 5.1 (exit code $LASTEXITCODE)." }
}
