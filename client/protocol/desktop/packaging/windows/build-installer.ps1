# Builds the Windows installer from the release zip that package-windows.ps1 made, so the installer
# carries exactly the zip's files:
#
#   .\build-installer.ps1 -Zip <nightjar-desktop-<version>-windows-x86_64.zip> [-Out <folder>]
#       [-Iscc <ISCC.exe>] [-SignCommand <command>] [-Smoke]
#
# The result is nightjar-desktop-<version>-windows-x86_64-setup.exe beside the zip (or in -Out).
# Needs Inno Setup 6 (ISCC.exe): -Iscc, $env:ISCC, or a usual install folder. The release workflow
# installs a pinned, hash-checked Inno Setup first.
#
# CODE SIGNING HOOK: there is no certificate yet, so nothing is signed. -SignCommand takes a sign
# command with $f standing for the file (for example
#   signtool sign /fd sha256 /tr http://timestamp.digicert.com /td sha256 /f cert.pfx /p ... $f
# ), and ISCC signs the installer and its uninstaller with it (nightjar.iss, SignTool=nightjarsign).
# nightjar.exe and the DLLs would be signed in package-windows.ps1 before the zip is packed, so the
# zip and the installer keep carrying the same files.
#
# -Smoke installs the result silently into a temporary folder, starts `nightjar.exe --version`
# from it, uninstalls it and checks that the files, the shortcuts and the uninstall entry are gone.
# Where %APPDATA%\Nightjar does not exist yet it also checks that an uninstall keeps the settings
# and that /REMOVESETTINGS removes them. It refuses to run where Nightjar is installed already (it
# would take over that install's uninstall entry): it is for CI runners and clean test PCs.
param(
    [Parameter(Mandatory)] [string]$Zip,
    [string]$Out = '',
    [string]$Iscc = '',
    [string]$SignCommand = '',
    [switch]$Smoke
)
$ErrorActionPreference = 'Stop'

$Zip = (Resolve-Path $Zip).Path
$zipName = [IO.Path]::GetFileNameWithoutExtension($Zip)
if ($zipName -notmatch '^nightjar-desktop-(.+)-windows-x86_64$') { throw "$Zip is not named nightjar-desktop-<version>-windows-x86_64.zip" }
$version = $Matches[1]
# Windows wants four numbers in the file version: 4.0.0-rc.1 becomes 4.0.0.0.
if ($version -notmatch '^(\d+)\.(\d+)\.(\d+)') { throw "version $version does not start with <major>.<minor>.<patch>" }
$numeric = "$($Matches[1]).$($Matches[2]).$($Matches[3]).0"
if (-not $Out) { $Out = Split-Path $Zip }
New-Item -ItemType Directory -Force $Out | Out-Null
$Out = (Resolve-Path $Out).Path

if (-not $Iscc) { $Iscc = $env:ISCC }
if (-not $Iscc) {
    $Iscc = @(
        "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
        "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
        "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
    ) | Where-Object { Test-Path $_ } | Select-Object -First 1
}
if (-not $Iscc) { $Iscc = (Get-Command ISCC.exe -ErrorAction SilentlyContinue).Source }
if (-not $Iscc -or -not (Test-Path $Iscc)) { throw 'ISCC.exe (Inno Setup 6) not found: pass -Iscc or set $env:ISCC' }

$stage = Join-Path ([IO.Path]::GetTempPath()) "nightjar-installer-$([guid]::NewGuid().ToString('N'))"
try {
    Expand-Archive -Path $Zip -DestinationPath $stage
    if (-not (Test-Path (Join-Path $stage 'nightjar.exe'))) { throw "$Zip has no nightjar.exe at its top" }
    $iscArgs = @('/Q', "/DAppVersion=$version", "/DNumericVersion=$numeric", "/DSourceDir=$stage",
        "/DOutputDir=$Out", "/DOutputBaseName=$zipName-setup")
    if ($SignCommand) { $iscArgs += '/DSign', "/Snightjarsign=$SignCommand" }
    & $Iscc @iscArgs (Join-Path $PSScriptRoot 'nightjar.iss')
    if ($LASTEXITCODE -ne 0) { throw "ISCC failed ($LASTEXITCODE)" }
} finally {
    Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
}
$setup = Join-Path $Out "$zipName-setup.exe"
if (-not (Test-Path $setup)) { throw "$setup was not built" }
'{0:N1} MB: {1}' -f ((Get-Item $setup).Length / 1MB), $setup

if (-not $Smoke) { return }

# The uninstall entry Inno Setup writes for nightjar.iss's AppId.
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{6A3C8F12-5E7B-4D2A-9C41-7B0E8D3F2A65}_is1'
if (Test-Path $uninstallKey) { throw "Nightjar is installed on this PC ($((Get-ItemProperty $uninstallKey).InstallLocation)); -Smoke is for clean machines" }
$dir = Join-Path ([IO.Path]::GetTempPath()) "nightjar-setup-smoke-$([guid]::NewGuid().ToString('N'))"
$startMenu = Join-Path ([Environment]::GetFolderPath('Programs')) 'Nightjar.lnk'
$desktop = Join-Path ([Environment]::GetFolderPath('DesktopDirectory')) 'Nightjar.lnk'
# The client's settings folder. The settings checks run only where it does not exist yet, so a
# developer's own settings are never touched.
$data = Join-Path ([Environment]::GetFolderPath('ApplicationData')) 'Nightjar'
$checkData = -not (Test-Path $data)

# Runs the installer or uninstaller and waits for it; Start-Process -Wait can lose the exit code.
function Invoke-Silent([string]$file, [string[]]$arguments) {
    $process = Start-Process $file -ArgumentList (@('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART') + $arguments) -PassThru
    $null = $process.Handle
    $process.WaitForExit()
    if ($process.ExitCode -ne 0) { throw "$([IO.Path]::GetFileName($file)) $($arguments -join ' ') exited with $($process.ExitCode)" }
}
# Uninstalls silently and checks that the files, the shortcuts and the uninstall entry are gone.
function Remove-Install([string[]]$arguments) {
    Invoke-Silent (Join-Path $dir 'unins000.exe') $arguments
    # The uninstaller removes its own files from a copy in %TEMP%, a moment after it returns.
    $deadline = (Get-Date).AddSeconds(30)
    while ((Test-Path $dir) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 250 }
    foreach ($path in $dir, $startMenu, $desktop, $uninstallKey) {
        if (Test-Path $path) { throw "the uninstall left $path behind" }
    }
}

Invoke-Silent $setup @("/DIR=`"$dir`"", "/LOG=`"$dir.install.log`"")
try {
    foreach ($path in (Join-Path $dir 'nightjar.exe'), (Join-Path $dir 'nightjar.ico'), (Join-Path $dir 'unins000.exe'), $startMenu, $desktop, $uninstallKey) {
        if (-not (Test-Path $path)) { throw "the install has no $path" }
    }
    $entry = Get-ItemProperty $uninstallKey
    if ($entry.DisplayVersion -ne $version -or $entry.Publisher -ne 'RamazanKara' -or -not $entry.DisplayIcon) {
        throw "the uninstall entry is incomplete: version '$($entry.DisplayVersion)', publisher '$($entry.Publisher)', icon '$($entry.DisplayIcon)'"
    }
    # Every file of the zip is installed, byte for byte.
    $unpacked = "$dir.zip"
    Expand-Archive -Path $Zip -DestinationPath $unpacked
    foreach ($file in Get-ChildItem $unpacked -File -Recurse) {
        $installed = Join-Path $dir $file.FullName.Substring($unpacked.Length + 1)
        if (-not (Test-Path $installed) -or (Get-FileHash $installed).Hash -ne (Get-FileHash $file.FullName).Hash) { throw "$installed differs from the zip" }
    }
    Remove-Item -Recurse -Force $unpacked
    # --version needs no window and no DLL beyond the package: a missing one ends it with 0xC0000135.
    $savedPath = $env:PATH
    $env:PATH = "$env:WINDIR\System32;$env:WINDIR"
    try {
        $versionOut = "$dir.version.txt"
        $run = Start-Process (Join-Path $dir 'nightjar.exe') -ArgumentList '--version' -RedirectStandardOutput $versionOut -PassThru
        $null = $run.Handle
        $run.WaitForExit()
        $printed = "$(Get-Content $versionOut -Raw)".Trim()
        if ($run.ExitCode -ne 0 -or $printed -ne "Nightjar $version") { throw ('installed nightjar.exe --version exited with 0x{0:X8} and printed "{1}"' -f $run.ExitCode, $printed) }
        Write-Host "smoke: installed $printed"
    } finally {
        $env:PATH = $savedPath
    }
    if ($checkData) { New-Item -ItemType File -Force (Join-Path $data 'config\settings.json') | Out-Null }
} catch {
    # Leave no install behind on a failed check.
    if (Test-Path (Join-Path $dir 'unins000.exe')) { Invoke-Silent (Join-Path $dir 'unins000.exe') @() }
    throw
}
Remove-Install @()
if ($checkData) {
    # A plain uninstall keeps the settings; /REMOVESETTINGS (the box in the dialog) removes them.
    if (-not (Test-Path (Join-Path $data 'config\settings.json'))) { throw "the uninstall removed $data without being asked to" }
    Invoke-Silent $setup @("/DIR=`"$dir`"")
    Remove-Install @('/REMOVESETTINGS')
    if (Test-Path $data) { throw "the uninstall with /REMOVESETTINGS left $data behind" }
    Write-Host 'smoke: the settings stay on uninstall and go with /REMOVESETTINGS'
} else {
    Write-Host "smoke: $data exists on this PC, so the settings checks were skipped"
}
Remove-Item -Force "$dir.install.log", "$dir.version.txt" -ErrorAction SilentlyContinue
Write-Host 'smoke: install, --version and uninstall passed'
