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
# -Smoke installs the result silently into a temporary folder, checks it against the zip, starts
# `nightjar.exe --version` from it, upgrades it in place with a second run (a stale DLL goes, the
# settings stay), uninstalls it and checks that the files, the shortcuts and the uninstall entry
# are gone. Then it checks the settings: a plain uninstall keeps them, /REMOVESETTINGS removes them
# but keeps the screenshots and the saved statistics. On GitHub Actions (a fresh runner) that runs
# with this installer against the real %APPDATA%\Nightjar; anywhere else the real folder is never
# created, moved or deleted, and a test installer built with /DTestDataDir=<temporary folder> runs
# the settings checks instead. It refuses to run where Nightjar is installed already (it would
# take over that install's uninstall entry and shortcuts): it is for CI runners and test PCs.
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

# Compiles nightjar.iss from the unpacked zip into $outDir and returns the installer's path.
function Build-Setup([string]$outDir, [string[]]$defines) {
    $stage = Join-Path ([IO.Path]::GetTempPath()) "nightjar-installer-$([guid]::NewGuid().ToString('N'))"
    try {
        Expand-Archive -Path $Zip -DestinationPath $stage
        if (-not (Test-Path (Join-Path $stage 'nightjar.exe'))) { throw "$Zip has no nightjar.exe at its top" }
        $iscArgs = @('/Q', "/DAppVersion=$version", "/DNumericVersion=$numeric", "/DSourceDir=$stage",
            "/DOutputDir=$outDir", "/DOutputBaseName=$zipName-setup") + $defines
        & $Iscc @iscArgs (Join-Path $PSScriptRoot 'nightjar.iss')
        if ($LASTEXITCODE -ne 0) { throw "ISCC failed ($LASTEXITCODE)" }
    } finally {
        Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
    }
    $built = Join-Path $outDir "$zipName-setup.exe"
    if (-not (Test-Path $built)) { throw "$built was not built" }
    $built
}

$setup = Build-Setup $Out $(if ($SignCommand) { @('/DSign', "/Snightjarsign=$SignCommand") } else { @() })
'{0:N1} MB: {1}' -f ((Get-Item $setup).Length / 1MB), $setup

if (-not $Smoke) { return }

# The uninstall entry Inno Setup writes for nightjar.iss's AppId.
$uninstallRoot = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall'
$uninstallKey = "$uninstallRoot\{6A3C8F12-5E7B-4D2A-9C41-7B0E8D3F2A65}_is1"
$startMenu = Join-Path ([Environment]::GetFolderPath('Programs')) 'Nightjar.lnk'
$desktop = Join-Path ([Environment]::GetFolderPath('DesktopDirectory')) 'Nightjar.lnk'
if (Test-Path $uninstallKey) { throw "Nightjar is installed on this PC ($((Get-ItemProperty $uninstallKey).InstallLocation)); -Smoke is for clean machines" }
foreach ($link in $startMenu, $desktop) {
    if (Test-Path $link) { throw "$link exists already: -Smoke would replace and then remove it" }
}
$dir = Join-Path ([IO.Path]::GetTempPath()) "nightjar-setup-smoke-$([guid]::NewGuid().ToString('N'))"
# Where the settings checks run (see the top). Only a fresh CI runner uses the real folder.
if ($env:GITHUB_ACTIONS -eq 'true') {
    $data = Join-Path ([Environment]::GetFolderPath('ApplicationData')) 'Nightjar'
    if (Test-Path $data) { throw "$data exists on this runner" }
    $dataSetup = $setup
} else {
    $data = "$dir.data"
    $dataSetup = Build-Setup "$dir.test-setup" @("/DTestDataDir=$data")
}
$config = Join-Path $data 'config'

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
    # Setup removes only a folder it made: one that was there before the install stays, empty.
    if ($script:dirWasThere -and (Test-Path $dir) -and -not (Get-ChildItem $dir -Force)) {
        Remove-Item $dir
        $script:dirWasThere = $false
    }
    foreach ($path in $dir, $startMenu, $desktop, $uninstallKey) {
        if (Test-Path $path) { throw "the uninstall left $path behind" }
    }
}

# Writes a small file, with its folder.
function New-TestFile([string]$path) {
    New-Item -ItemType Directory -Force (Split-Path $path) | Out-Null
    Set-Content -NoNewline -Path $path -Value "nightjar smoke: $([IO.Path]::GetFileName($path))"
}
# Every file of the zip is installed, byte for byte.
function Assert-MatchesZip {
    $unpacked = "$dir.zip"
    Remove-Item -Recurse -Force $unpacked -ErrorAction SilentlyContinue
    Expand-Archive -Path $Zip -DestinationPath $unpacked
    foreach ($file in Get-ChildItem $unpacked -File -Recurse) {
        $installed = Join-Path $dir $file.FullName.Substring($unpacked.Length + 1)
        if (-not (Test-Path $installed) -or (Get-FileHash $installed).Hash -ne (Get-FileHash $file.FullName).Hash) { throw "$installed differs from the zip" }
    }
    Remove-Item -Recurse -Force $unpacked
}
# One Apps & features entry, for this version and folder.
function Assert-Entry {
    $entries = @(Get-ChildItem $uninstallRoot | ForEach-Object { Get-ItemProperty $_.PSPath } | Where-Object { $_.DisplayName -eq 'Nightjar' })
    if ($entries.Count -ne 1) { throw "there are $($entries.Count) Nightjar entries in Apps & features" }
    $entry = Get-ItemProperty $uninstallKey
    if ($entry.DisplayVersion -ne $version -or $entry.Publisher -ne 'RamazanKara' -or -not $entry.DisplayIcon -or "$($entry.InstallLocation)".TrimEnd('\') -ne $dir) {
        throw "the uninstall entry is wrong: version '$($entry.DisplayVersion)', publisher '$($entry.Publisher)', icon '$($entry.DisplayIcon)', folder '$($entry.InstallLocation)'"
    }
}
$settings = Join-Path $config 'settings.json'
# The client's files and the user's own files in its folder (config.rs, main.rs, stream/mod.rs).
$clientFiles = 'settings.json', 'hosts.json', 'identity.json', 'art\pc\cover.png', 'library\pc.json', 'logs\nightjar.log'
$userFiles = 'screenshots\nightjar-1.png', 'statistics\nightjar-1.csv'

# Settings from before: neither the install nor the upgrade touches them.
New-TestFile $settings
$settingsHash = (Get-FileHash $settings).Hash
try {
    # A first install into a folder that holds a DLL of something else (/DIR) leaves it alone: only
    # an upgrade of Nightjar's own folder clears out old DLLs.
    $foreign = Join-Path $dir 'foreign.dll'
    New-TestFile $foreign
    $script:dirWasThere = $true
    Invoke-Silent $setup @("/DIR=`"$dir`"", "/LOG=`"$dir.install.log`"")
    if (-not (Test-Path $foreign)) { throw "a first install into $dir deleted a DLL that was there before" }
    Remove-Item $foreign
    foreach ($path in (Join-Path $dir 'nightjar.exe'), (Join-Path $dir 'nightjar.ico'), (Join-Path $dir 'unins000.exe'), $startMenu, $desktop, $uninstallKey) {
        if (-not (Test-Path $path)) { throw "the install has no $path" }
    }
    Assert-Entry
    Assert-MatchesZip
    # --version needs no window and no DLL beyond the package: a missing one ends it with 0xC0000135.
    $savedPath = $env:PATH
    $savedConfig = $env:NIGHTJAR_CONFIG_DIR
    $env:PATH = "$env:WINDIR\System32;$env:WINDIR"
    $env:NIGHTJAR_CONFIG_DIR = "$dir.config"
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
        $env:NIGHTJAR_CONFIG_DIR = $savedConfig
    }

    # An update: Setup again over the live install, without /DIR. A DLL the package does not ship
    # (an older FFmpeg) goes; the settings stay.
    New-TestFile (Join-Path $dir 'stale.dll')
    Invoke-Silent $setup @("/LOG=`"$dir.upgrade.log`"")
    if (Test-Path (Join-Path $dir 'stale.dll')) { throw 'the upgrade left stale.dll in the install' }
    Assert-Entry
    Assert-MatchesZip
    if ((Get-FileHash $settings).Hash -ne $settingsHash) { throw "the install or the upgrade changed $settings" }
    Write-Host 'smoke: upgraded in place (stale DLL gone, files match the zip, one uninstall entry, settings kept)'
    Remove-Install @()
    Write-Host 'smoke: uninstalled, nothing left'

    # The settings, with $dataSetup (see the top): a plain uninstall keeps everything.
    Invoke-Silent $dataSetup @("/DIR=`"$dir`"")
    foreach ($file in $clientFiles + $userFiles) { New-TestFile (Join-Path $config $file) }
    Remove-Install @()
    foreach ($file in $clientFiles + $userFiles) {
        if (-not (Test-Path (Join-Path $config $file))) { throw "a plain uninstall removed $file from $config" }
    }
    # /REMOVESETTINGS (the box in the dialog) removes the client's files and keeps the user's own.
    Invoke-Silent $dataSetup @("/DIR=`"$dir`"")
    Remove-Install @('/REMOVESETTINGS')
    $left = @(Get-ChildItem $config -Recurse -File | ForEach-Object { $_.FullName.Substring($config.Length + 1) } | Sort-Object)
    if (($left -join ', ') -ne (($userFiles | Sort-Object) -join ', ')) { throw "/REMOVESETTINGS left $config with: $($left -join ', ')" }
    # Without files of the user's own, the folder goes as a whole.
    Remove-Item -Recurse -Force (Join-Path $config 'screenshots'), (Join-Path $config 'statistics')
    New-TestFile $settings
    Invoke-Silent $dataSetup @("/DIR=`"$dir`"")
    Remove-Install @('/REMOVESETTINGS')
    if (Test-Path $data) { throw "/REMOVESETTINGS left $data behind" }
    Write-Host 'smoke: the settings stay on uninstall; /REMOVESETTINGS removes them and keeps screenshots and statistics'
} catch {
    # Leave no install behind on a failed check (and keep the first error).
    $failure = $_
    try {
        if (Test-Path $uninstallKey) {
            $uninstaller = Join-Path (Get-ItemProperty $uninstallKey).InstallLocation 'unins000.exe'
            if (Test-Path $uninstaller) { Invoke-Silent $uninstaller @() }
        }
    } catch {
        Write-Warning "could not remove the smoke install: $_"
    }
    throw $failure
} finally {
    Remove-Item -Recurse -Force "$dir.install.log", "$dir.upgrade.log", "$dir.version.txt", "$dir.config", "$dir.zip", "$dir.test-setup" -ErrorAction SilentlyContinue
    # Only the temporary folder: never the real %APPDATA%\Nightjar.
    if ($data -eq "$dir.data") { Remove-Item -Recurse -Force $data -ErrorAction SilentlyContinue }
}
Write-Host 'smoke: passed'
