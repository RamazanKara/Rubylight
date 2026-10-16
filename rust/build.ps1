[CmdletBinding()]
param(
    [string]$MsysRoot = 'C:\msys64',
    [string]$Dependencies = "$env:LOCALAPPDATA\ButterpolloRust\sdk",
    [string]$TargetDirectory = "$env:LOCALAPPDATA\ButterpolloRust\target",
    [string]$FfmpegRoot = $env:BUTTERPOLLO_FFMPEG_ROOT,
    [string]$PyrowaveRoot = $env:BUTTERPOLLO_PYROWAVE_ROOT,
    # Signed virtual display and gamepad driver packages (a Vibepollo
    # installation's drivers folder); fetched from Vibepollo 2.0.0 otherwise.
    [string]$DriverRoot = $env:BUTTERPOLLO_DRIVER_ROOT,
    [switch]$FetchDependencies,
    [switch]$DebugBuild,
    [switch]$SkipTests,
    [switch]$Package
)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).ProviderPath
$toolchain = '+1.98.1-x86_64-pc-windows-gnu'
function Assert-NativeExit([string]$Operation) {
    if ($LASTEXITCODE -ne 0) { throw "$Operation failed with exit code $LASTEXITCODE" }
}
function Get-PinnedArchive([string]$Url, [string]$Destination, [string]$Hash) {
    if (!(Test-Path -LiteralPath $Destination)) { Invoke-WebRequest -Uri $Url -OutFile $Destination -TimeoutSec 180 }
    if ((Get-FileHash -LiteralPath $Destination -Algorithm SHA256).Hash.ToLowerInvariant() -ne $Hash) {
        throw "Checksum mismatch: $Destination"
    }
}
New-Item -ItemType Directory -Path $Dependencies, $TargetDirectory -Force | Out-Null
$Dependencies = (Resolve-Path -LiteralPath $Dependencies).ProviderPath
$TargetDirectory = (Resolve-Path -LiteralPath $TargetDirectory).ProviderPath
$env:PATH = "$MsysRoot\ucrt64\bin;$env:USERPROFILE\.cargo\bin;" + $env:PATH
$env:LIBCLANG_PATH = "$MsysRoot\ucrt64\bin"
$env:BUTTERPOLLO_SYSTEM_LIBS = "$MsysRoot\ucrt64\lib"
$env:BUTTERPOLLO_VULKAN_INCLUDE = "$MsysRoot\ucrt64\include"
$env:CARGO_TARGET_DIR = $TargetDirectory
if ($FetchDependencies) {
    if (!$FfmpegRoot) {
        $archive = Join-Path $Dependencies 'ffmpeg-v2026.516.30821.tar.gz'
        Get-PinnedArchive 'https://github.com/LizardByte/build-deps/releases/download/v2026.516.30821/Windows-AMD64-ffmpeg.tar.gz' $archive '2f7a2c2fc6be9b96de3c6f654389f73a5e5d369d7e802d017894fae96247661d'
        $directory = Join-Path $Dependencies 'ffmpeg-v2026.516.30821'
        New-Item -ItemType Directory -Path $directory -Force | Out-Null
        & tar -xzf $archive -C $directory
        Assert-NativeExit 'FFmpeg extraction'
        $FfmpegRoot = (Get-ChildItem -LiteralPath $directory -Filter avcodec.h -Recurse | Select-Object -First 1).Directory.Parent.Parent.FullName
    }
    if (!$PyrowaveRoot) {
        $PyrowaveRoot = Join-Path $Dependencies 'pyrowave-502a3b52'
        $cygpath = "$MsysRoot\usr\bin\cygpath.exe"
        $prefix = & $cygpath -u $PyrowaveRoot
        $buildScript = & $cygpath -u (Join-Path $repo 'rust\tools\build_pyrowave.sh')
        $env:PYROWAVE_WORKDIR = & $cygpath -u (Join-Path $Dependencies 'pyrowave-work')
        $env:MSYSTEM = 'UCRT64'
        & "$MsysRoot\usr\bin\bash.exe" $buildScript '502a3b52a39312ab82c85b1e2fc0e746faee91a4' $prefix
        Assert-NativeExit 'PyroWave SDK build'
    }
}
if ($FetchDependencies -and !$DriverRoot) {
    # The gamepad driver (libvirtualgamepad 0.1.0-beta.6, MIT), signed for
    # Vibepollo 2.0.0 by the SignPath Foundation, and nefconc.exe. Upstream
    # archives are unsigned, so take them from the release.
    $setup = Join-Path $Dependencies 'VibepolloSetup-v2.0.0.exe'
    Get-PinnedArchive 'https://github.com/Nonary/Vibepollo/releases/download/2.0.0/VibepolloSetup-v2.0.0.exe' $setup '7b3500ec0c774644ce5a435a48f61c046c48494d0f18b67afa0b3561931794b7'
    $msi = Join-Path $Dependencies 'vibepollo-2.0.0-payload.msi'
    $resource = [Reflection.Assembly]::LoadFile($setup).GetManifestResourceStream('Payload.msi')
    $file = [IO.File]::Create($msi)
    try { $resource.CopyTo($file) } finally { $file.Dispose(); $resource.Dispose() }
    $expanded = Join-Path $Dependencies 'vibepollo-2.0.0-msi'
    if (Test-Path -LiteralPath $expanded) { Remove-Item -LiteralPath $expanded -Recurse -Force }
    $process = Start-Process msiexec.exe -ArgumentList "/a `"$msi`" /qn TARGETDIR=`"$expanded`"" -Wait -PassThru
    if ($process.ExitCode -ne 0) { throw "Extracting the Vibepollo driver packages failed with $($process.ExitCode)" }
    $DriverRoot = Join-Path $expanded 'Apollo\drivers'
}
if ($DriverRoot) {
    foreach ($catalog in @('vhf-gamepad\driver\VibeshineVhfGamepad.cat', 'vhf-gamepad\tools\VibeshineVhfGamepadDeviceSetup.exe')) {
        $signature = Get-AuthenticodeSignature -LiteralPath (Join-Path $DriverRoot $catalog)
        if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch '^CN=SignPath Foundation') { throw "Driver file is not signed as expected: $catalog" }
    }
}
if (!$FfmpegRoot -or !$PyrowaveRoot) { throw 'Provide FFmpeg/PyroWave SDK paths, or use -FetchDependencies' }
$pyrowaveInfo = Join-Path $PyrowaveRoot 'share\pyrowave-shared\build-info.txt'
if (!(Test-Path -LiteralPath $pyrowaveInfo)) { throw 'PyroWave SDK build-info.txt is missing; rebuild using -FetchDependencies' }
$pyrowaveIdentity = Get-Content -LiteralPath $pyrowaveInfo -Raw
foreach ($identity in @(
    'pyrowave_commit=502a3b52a39312ab82c85b1e2fc0e746faee91a4',
    'granite_commit=fb178c8080d163419e8d20f10715c61c53c1ec9b',
    'patches=0002-payload-data-444-sizing,0003-decoder-reject-short-block'
)) {
    if ($pyrowaveIdentity -notmatch ('(?m)^' + [regex]::Escape($identity) + '\r?$')) {
        throw "PyroWave SDK does not match the pinned build: expected $identity. Rebuild using -FetchDependencies without -PyrowaveRoot."
    }
}
$env:BUTTERPOLLO_FFMPEG_ROOT = $FfmpegRoot
$env:BUTTERPOLLO_PYROWAVE_ROOT = $PyrowaveRoot
Push-Location -LiteralPath $repo
try {
    & cargo $toolchain fmt --all -- --check
    Assert-NativeExit 'Rust formatting'
    if (!$SkipTests) {
        & (Join-Path $repo 'rust\drivers\usbip\test_usbip.ps1')
        & cargo $toolchain test --workspace --locked
        Assert-NativeExit 'Rust tests'
        & cargo $toolchain clippy --workspace --all-targets --locked -- -D warnings
        Assert-NativeExit 'Rust lint'
    }
    $buildArgs = @('build', '--workspace', '--locked')
    $profile = 'debug'
    if (!$DebugBuild) { $buildArgs += '--release'; $profile = 'release' }
    & cargo $toolchain @buildArgs
    Assert-NativeExit 'Rust host build'
    $probeArgs = @('build', '-p', 'butterpollo-windows', '--example', 'performance', '--locked')
    if (!$DebugBuild) { $probeArgs += '--release' }
    & cargo $toolchain @probeArgs
    Assert-NativeExit 'Rust performance probe build'
    $protocolArgs = @('build', '-p', 'butterpollo-core', '--example', 'protocol_performance', '--example', 'video_packet_performance', '--locked')
    if (!$DebugBuild) { $protocolArgs += '--release' }
    & cargo $toolchain @protocolArgs
    Assert-NativeExit 'Rust protocol performance probe build'
    $output = Join-Path $TargetDirectory $profile
    $runtimeDlls = @('libopus-0.dll','libvpl-2.dll','libstdc++-6.dll','libgcc_s_seh-1.dll','libwinpthread-1.dll','libpyrowave-shared-1.dll')
    foreach ($dll in $runtimeDlls | Where-Object { $_ -ne 'libpyrowave-shared-1.dll' }) {
        Copy-Item -LiteralPath (Join-Path "$MsysRoot\ucrt64\bin" $dll) -Destination $output
    }
    Copy-Item -LiteralPath (Join-Path $PyrowaveRoot 'bin\libpyrowave-shared-1.dll') -Destination $output
    if ($Package) {
        $distribution = Join-Path $TargetDirectory "butterpollo-rust-$profile"
        if (Test-Path -LiteralPath $distribution) {
            $resolvedDistribution = (Resolve-Path -LiteralPath $distribution).ProviderPath
            if (!$resolvedDistribution.StartsWith($TargetDirectory.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Package cleanup escaped its output directory' }
            Remove-Item -LiteralPath $resolvedDistribution -Recurse -Force
        }
        New-Item -ItemType Directory -Path "$distribution\assets\web", "$distribution\licenses" -Force | Out-Null
        foreach ($exe in @('butterpollo.exe', 'butterpollo-service.exe', 'butterpollo-setup.exe')) { Copy-Item -LiteralPath (Join-Path $output $exe) -Destination $distribution }
        Copy-Item -LiteralPath (Join-Path $output 'butterpollo-start.exe') -Destination (Join-Path $distribution 'Start Rubylight.exe')
        Copy-Item -LiteralPath (Join-Path $PyrowaveRoot 'share\pyrowave-shared\build-info.txt') -Destination "$distribution\licenses\pyrowave-build-info.txt"
        Copy-Item -LiteralPath (Join-Path $output 'examples\performance.exe') -Destination (Join-Path $distribution 'butterpollo-performance.exe')
        Copy-Item -LiteralPath (Join-Path $output 'examples\protocol_performance.exe') -Destination (Join-Path $distribution 'butterpollo-protocol-performance.exe')
        Copy-Item -LiteralPath (Join-Path $output 'examples\video_packet_performance.exe') -Destination (Join-Path $distribution 'butterpollo-video-packet-performance.exe')
        foreach ($dll in $runtimeDlls) { Copy-Item -LiteralPath (Join-Path $output $dll) -Destination $distribution }
        New-Item -ItemType Directory -Path "$distribution\vulkan-layer" -Force | Out-Null
        Copy-Item -LiteralPath (Join-Path $output 'butterpollo_vulkan_layer.dll') -Destination "$distribution\vulkan-layer"
        Copy-Item -LiteralPath (Join-Path $repo 'rust\vulkan-layer\VkLayer_butterpollo_hdr.json') -Destination "$distribution\vulkan-layer"
        Get-ChildItem -LiteralPath (Join-Path $repo 'rust\assets\package') -File | Where-Object { $_.Extension -in '.png','.ico' } | Copy-Item -Destination "$distribution\assets"
        Copy-Item -LiteralPath (Join-Path $repo 'rust\assets\package\remote-session') -Destination "$distribution\assets\remote-session" -Recurse -Force
        # The web console, built in a copy: node_modules made by a WSL checkout
        # holds Linux binaries.
        $webBuild = Join-Path $TargetDirectory 'web-build'
        & robocopy (Join-Path $repo 'rust\web') $webBuild /MIR /XD node_modules dist /NFL /NDL /NJH /NJS /NP | Out-Null
        if ($LASTEXITCODE -ge 8) { throw "Copying the web console failed with $LASTEXITCODE" }
        Push-Location -LiteralPath $webBuild
        try {
            & npm ci --no-audit --no-fund
            Assert-NativeExit 'Web console dependencies'
            & npm run build
            Assert-NativeExit 'Web console build'
        } finally { Pop-Location }
        Copy-Item -Path (Join-Path $webBuild 'dist\*') -Destination "$distribution\assets\web" -Recurse -Force
        Copy-Item -LiteralPath (Join-Path $repo 'LICENSE') -Destination "$distribution\licenses\Butterpollo.txt"
        Copy-Item -LiteralPath (Join-Path $repo 'rust\README.md') -Destination $distribution
        Copy-Item -LiteralPath (Join-Path $repo 'rust\RELEASE_NOTES.md') -Destination $distribution
        Copy-Item -LiteralPath (Join-Path $repo 'rust\PERFORMANCE.md') -Destination $distribution
        Copy-Item -LiteralPath (Join-Path $repo 'docs\features.md') -Destination $distribution
        New-Item -ItemType Directory -Path "$distribution\tools" -Force | Out-Null
        Copy-Item -LiteralPath (Join-Path $repo 'rust\tests\collect_environment.ps1') -Destination "$distribution\tools"
        Copy-Item -LiteralPath (Join-Path $repo 'rust\tests\ENVIRONMENT_REPORT.md') -Destination "$distribution\tools\README.md"
        Copy-Item -LiteralPath (Join-Path $repo 'rust\compatibility') -Destination "$distribution\compatibility" -Recurse
        Copy-Item -LiteralPath (Join-Path $repo 'rust\THIRD_PARTY.md') -Destination "$distribution\licenses"
        Copy-Item -LiteralPath (Join-Path $repo 'rust\service.ps1') -Destination $distribution
        Copy-Item -LiteralPath (Join-Path $repo 'Cargo.lock') -Destination "$distribution\licenses"
        if (Test-Path -LiteralPath (Join-Path $PyrowaveRoot 'share\licenses')) { Copy-Item -LiteralPath (Join-Path $PyrowaveRoot 'share\licenses') -Destination "$distribution\licenses\pyrowave" -Recurse -Force }
        foreach ($library in @('gcc-libs','libwinpthread','winpthreads','libvpl','opus','svt-av1')) {
            $source = Join-Path "$MsysRoot\ucrt64\share\licenses" $library
            if (Test-Path -LiteralPath $source) { Copy-Item -LiteralPath $source -Destination "$distribution\licenses\$library" -Recurse -Force }
        }
        if ($DriverRoot) {
            # Setup installs the gamepad driver with Vibepollo's own scripts.
            New-Item -ItemType Directory -Path "$distribution\drivers" -Force | Out-Null
            Copy-Item -LiteralPath (Join-Path $DriverRoot 'vhf-gamepad') -Destination "$distribution\drivers\gamepad" -Recurse -Force
            # The Playnite plugin from the same release; the host installs it
            # into Playnite.
            $playnitePlugin = Join-Path (Split-Path -Parent $DriverRoot) 'plugins\playnite\SunshinePlaynite'
            if (Test-Path -LiteralPath $playnitePlugin) {
                New-Item -ItemType Directory -Path "$distribution\plugins\playnite" -Force | Out-Null
                Copy-Item -LiteralPath $playnitePlugin -Destination "$distribution\plugins\playnite\SunshinePlaynite" -Recurse -Force
            }
            @(
                'Virtual gamepad driver: https://github.com/Nonary/libvirtualgamepad v0.1.0-beta.6 (MIT).',
                'Driver catalog, tools and install scripts as released in Vibepollo 2.0.0 (GPL-3.0),',
                'https://github.com/Nonary/Vibepollo, signed by the SignPath Foundation.',
                'Playnite plugin (plugins\playnite): Sunshine Playnite Connector from the same Vibepollo 2.0.0 release (GPL-3.0).',
                'nefconc.exe: https://github.com/nefarius/nefcon by Nefarius Software Solutions.'
            ) | Set-Content -LiteralPath "$distribution\licenses\drivers.txt" -Encoding utf8
        }
        # Rubylight's own display driver, unsigned: setup installs it with
        # install-local.ps1, signing the catalog on the user's PC, and removes
        # the libvirtualdisplay driver earlier versions installed. A signed release replaces the catalog with SignPath's
        # (rust/release/sign-artifacts.ps1), which install-local.ps1 then keeps.
        # A child PowerShell: the driver's MSVC environment stays out of this one.
        # DriverVer is the release (2.4.0-rc.1 is 2.4.0) plus the build hour, so a release's
        # driver outranks the 0.1.0.x development builds and each earlier release.
        $vdisplay = Join-Path $repo 'rust\drivers\rubylight-vdisplay'
        $release = (Select-String -LiteralPath (Join-Path $repo 'Cargo.toml') -Pattern '^version = "(\d+)\.(\d+)\.(\d+)').Matches[0].Groups
        $hour = [int]((Get-Date).ToUniversalTime() - [datetime]'2026-01-01').TotalHours
        $driverVersion = "$($release[1].Value).$($release[2].Value).$($release[3].Value).$hour"
        & (Get-Process -Id $PID).Path -NoProfile -ExecutionPolicy Bypass -File (Join-Path $vdisplay 'build-package.ps1') -Version $driverVersion
        Assert-NativeExit 'Display driver build'
        $displayDriver = Join-Path $distribution 'drivers\rubylight-display'
        New-Item -ItemType Directory -Path $displayDriver -Force | Out-Null
        Copy-Item -Path (Join-Path $vdisplay 'target\package\*') -Destination $displayDriver
        Copy-Item -LiteralPath (Join-Path $vdisplay 'install-local.ps1') -Destination $displayDriver
        Copy-Item -LiteralPath (Join-Path $repo 'rust\setup\remove-libvirtualdisplay.ps1') -Destination $displayDriver
        # install-local.ps1 creates the device with it on a PC that never had one.
        if (!$DriverRoot) { throw 'The display driver needs nefconc.exe: pass -DriverRoot or -FetchDependencies' }
        Copy-Item -LiteralPath (Join-Path $DriverRoot 'sunshine\nefconc.exe') -Destination $displayDriver
        Copy-Item -LiteralPath (Join-Path $vdisplay 'LICENSE-MIT') -Destination "$distribution\licenses\rubylight-display-MIT.txt"
        # The shared USB/IP installer is carried offline in both packages.
        # Bundling validates the original release without running it.
        & (Join-Path $repo 'rust\drivers\usbip\bundle.ps1') -Cache $Dependencies `
            -Destination (Join-Path $distribution 'drivers\usbip')
        Copy-Item -LiteralPath (Join-Path $repo 'rust\drivers\usbip\LICENSE.txt') -Destination "$distribution\licenses\usbip-win2.txt"
        $metadata = & cargo $toolchain metadata --format-version 1 --locked
        Assert-NativeExit 'Dependency manifest'
        ($metadata | ConvertFrom-Json).packages | Select-Object name, version, license, repository, source | ConvertTo-Json | Set-Content -LiteralPath "$distribution\licenses\rust-dependencies.json" -Encoding utf8
        $version = (Select-String -LiteralPath (Join-Path $repo 'Cargo.toml') -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
        & (Join-Path $repo 'rust\release\package.ps1') -Distribution $distribution -TargetDirectory $TargetDirectory `
            -Stub (Join-Path $output 'butterpollo-setup.exe') -Version $version -ProfileName $profile
    }
    Write-Output "Rust executables: $output"
} finally { Pop-Location }
