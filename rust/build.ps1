[CmdletBinding()]
param(
    [string]$MsysRoot = 'C:\msys64',
    [string]$Dependencies = "$env:LOCALAPPDATA\ButterpolloRust\sdk",
    [string]$TargetDirectory = "$env:LOCALAPPDATA\ButterpolloRust\target",
    [string]$PyrowaveRoot = $env:BUTTERPOLLO_PYROWAVE_ROOT,
    # FFmpeg's LGPL AC3 and E-AC3 codecs for audio passthrough (rust/tools/build_ffmpeg.sh).
    [string]$FfmpegRoot = $env:BUTTERPOLLO_FFMPEG_ROOT,
    # Signed virtual display and gamepad driver packages (a Vibepollo
    # installation's drivers folder, whose gamepad driver must be the pinned
    # libvirtualgamepad 0.1.0-beta.10); fetched from Vibepollo 2.0.0 and,
    # for the gamepad driver, 2.0.6 otherwise.
    [string]$DriverRoot = $env:BUTTERPOLLO_DRIVER_ROOT,
    # The separate, unsigned NVIDIA build: the host with the nvidia-proprietary
    # feature, the NGX adapter (rust/truehdr-runtime, linking NVIDIA's RTX Video
    # SDK) and NVIDIA's runtime DLLs, packaged under names with "nvidia". Signed
    # releases never use it (docs/code-signing-policy.md).
    [switch]$Nvidia,
    # An extracted RTX Video SDK 1.1.0; fetched and checked when not given.
    [string]$NvidiaRoot = $env:NV_RTX_VIDEO_SDK,
    # An MSVC CRT and Windows SDK folder for the adapter, outside a VS developer shell.
    [string]$MsvcSdk = $env:BUTTERPOLLO_MSVC_ROOT,
    [switch]$FetchDependencies,
    [switch]$DebugBuild,
    [switch]$SkipTests,
    [switch]$Package,
    # CI runs the stages in parallel jobs: Test (tests), Lint (clippy) and
    # Build (the release build and -Package). All runs them in order.
    [ValidateSet('All', 'Test', 'Lint', 'Build')]
    [string]$Stage = 'All',
    [ValidateRange(1, 256)][int]$Jobs = 4
)
$ErrorActionPreference = 'Stop'
if ($Package -and $env:RLVD_IDDCX_LIB) {
    # A driver built against that IddCx stub is probe-only: IddCxDeviceInitConfig rejects its client config.
    throw 'RLVD_IDDCX_LIB is set (a probe-only driver build); unset it before packaging'
}
$repo = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).ProviderPath
# Stay in this shell so callers retain the SDK environment, as before.
. (Join-Path $repo 'rust\release\check_streams.ps1')
$lockDirectory = if ($env:RUBYLIGHT_LOCK_DIRECTORY) { $env:RUBYLIGHT_LOCK_DIRECTORY } else { 'C:\src\butterpollo-release' }
Initialize-CheckLocks $lockDirectory
$buildLease = Enter-CheckBuildLock -RespectPriority -Owner "build.ps1 $Stage $repo"
$previousBuildOwner = $env:RUBYLIGHT_BUILD_LOCK_OWNER
$previousBuildPath = $env:RUBYLIGHT_BUILD_LOCK_PATH
$env:RUBYLIGHT_BUILD_LOCK_OWNER = Read-Lock $script:CheckBuildLock
$env:RUBYLIGHT_BUILD_LOCK_PATH = [IO.Path]::GetFullPath($script:CheckBuildLock)
try {
$env:CARGO_BUILD_JOBS = [string]$Jobs
$env:CMAKE_BUILD_PARALLEL_LEVEL = [string]$Jobs
$env:PYROWAVE_JOBS = [string]$Jobs
$env:FFMPEG_JOBS = [string]$Jobs
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
$env:BUTTERPOLLO_VULKAN_INCLUDE = "$MsysRoot\ucrt64\include"
$env:CARGO_TARGET_DIR = $TargetDirectory
if ($FetchDependencies) {
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
    if (!$FfmpegRoot) {
        # The official source release, built with only the AC3 and E-AC3 codecs (LGPL).
        $FfmpegRoot = Join-Path $Dependencies 'ffmpeg-8.1.3'
        $tarball = Join-Path $Dependencies 'ffmpeg-8.1.3.tar.xz'
        $ffmpegSha256 = '7138d28c96d9d3e3af4ee3d8cad72741f8ffb40da90c1112235dea3ecd3178a3'
        Get-PinnedArchive 'https://ffmpeg.org/releases/ffmpeg-8.1.3.tar.xz' $tarball $ffmpegSha256
        $cygpath = "$MsysRoot\usr\bin\cygpath.exe"
        $env:FFMPEG_WORKDIR = & $cygpath -u (Join-Path $Dependencies 'ffmpeg-work')
        # Checked above; MSYS sha256sum fails in a scheduled task without a console.
        $env:FFMPEG_SOURCE_SHA256 = $ffmpegSha256
        $env:MSYSTEM = 'UCRT64'
        & "$MsysRoot\usr\bin\bash.exe" (& $cygpath -u (Join-Path $repo 'rust\tools\build_ffmpeg.sh')) (& $cygpath -u $tarball) (& $cygpath -u $FfmpegRoot)
        Assert-NativeExit 'FFmpeg AC3 build'
    }
}
if ($Nvidia -and !$NvidiaRoot) {
    # NVIDIA's RTX Video SDK 1.1.0 (TrueHDR and VSR), pinned by hash.
    $archive = Join-Path $Dependencies 'RTX_Video_SDK_v1.1.0.zip'
    if (!(Test-Path -LiteralPath $archive)) {
        $redirect = Invoke-WebRequest 'https://api.ngc.nvidia.com/v2/models/nvidia/multimedia/dlpp/versions/1.5/files/RTX_Video_SDK_v1.1.0.zip' -TimeoutSec 180
        Invoke-WebRequest -Uri ([string]$redirect.Headers.Location) -OutFile $archive -TimeoutSec 180
    }
    if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne 'abf4f34e2b5a618e355b0d5a0365d8ecc3db4396e756e4c850a867e1ae2ed69e') {
        throw "Checksum mismatch: $archive"
    }
    $NvidiaRoot = Join-Path $Dependencies 'rtx-video-sdk-1.1.0'
    if (!(Test-Path -LiteralPath (Join-Path $NvidiaRoot 'include\nvsdk_ngx_defs_vsr.h'))) {
        Expand-Archive -LiteralPath $archive -DestinationPath $NvidiaRoot -Force
    }
}
# The host's NVIDIA feature, for every cargo step of an NVIDIA build.
$featureArgs = if ($Nvidia) { @('--features', 'butterpollo/nvidia-proprietary') } else { @() }
$flavor = if ($Nvidia) { 'nvidia-' } else { '' }
$runTests = !$SkipTests -and $Stage -in 'All', 'Test'
$runLint = !$SkipTests -and $Stage -in 'All', 'Lint'
$runBuild = $Stage -in 'All', 'Build'
if ($Package -and !$runBuild) { throw "-Package needs the Build stage, not $Stage" }
# Only the package carries the drivers.
function Expand-VibepolloDrivers([string]$Version, [string]$Hash) {
    $setup = Join-Path $Dependencies "VibepolloSetup-v$Version.exe"
    Get-PinnedArchive "https://github.com/Nonary/Vibepollo/releases/download/$Version/VibepolloSetup-v$Version.exe" $setup $Hash
    $msi = Join-Path $Dependencies "vibepollo-$Version-payload.msi"
    $resource = [Reflection.Assembly]::LoadFile($setup).GetManifestResourceStream('Payload.msi')
    $file = [IO.File]::Create($msi)
    try { $resource.CopyTo($file) } finally { $file.Dispose(); $resource.Dispose() }
    $expanded = Join-Path $Dependencies "vibepollo-$Version-msi"
    if (Test-Path -LiteralPath $expanded) { Remove-Item -LiteralPath $expanded -Recurse -Force }
    $process = Start-Process msiexec.exe -ArgumentList "/a `"$msi`" /qn TARGETDIR=`"$expanded`"" -Wait -PassThru
    if ($process.ExitCode -ne 0) { throw "Extracting the Vibepollo $Version driver packages failed with $($process.ExitCode)" }
    Join-Path $expanded 'Apollo\drivers'
}
$gamepadRoot = $null
if ($FetchDependencies -and !$DriverRoot -and $runBuild) {
    # nefconc.exe and the Playnite plugin from Vibepollo 2.0.0, and the
    # gamepad driver (libvirtualgamepad 0.1.0-beta.10, MIT) as signed for
    # Vibepollo 2.0.6 by the SignPath Foundation. Upstream archives are
    # unsigned, so take them from the releases.
    $DriverRoot = Expand-VibepolloDrivers '2.0.0' '7b3500ec0c774644ce5a435a48f61c046c48494d0f18b67afa0b3561931794b7'
    $gamepadRoot = Join-Path (Expand-VibepolloDrivers '2.0.6' 'c488977ac49e391dfd963d97439a01a9c56d4ff21a6061226adb6d69d0aae9e4') 'vhf-gamepad'
}
if ($DriverRoot) {
    if (!$gamepadRoot) { $gamepadRoot = Join-Path $DriverRoot 'vhf-gamepad' }
    foreach ($catalog in @('driver\VibeshineVhfGamepad.cat', 'tools\VibeshineVhfGamepadDeviceSetup.exe')) {
        $signature = Get-AuthenticodeSignature -LiteralPath (Join-Path $gamepadRoot $catalog)
        if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch '^CN=SignPath Foundation') { throw "Driver file is not signed as expected: $catalog" }
    }
    # The files libvirtualgamepad v0.1.0-beta.10 released (its manifest.json),
    # which the signed catalog covers.
    foreach ($pinned in @(
        @('driver\VibeshineVhfGamepad.dll', 'd5075fc7f74e9c7ebe6abc46b4db9a6ae690885315ea9538ec3610891dbc676b'),
        @('driver\VibeshineVhfGamepad.inf', '01fe8a807b0f19c8b10854d24ccd0d46c5320a1705c16a3c44b0c304dbdcf48d')
    )) {
        if ((Get-FileHash -LiteralPath (Join-Path $gamepadRoot $pinned[0]) -Algorithm SHA256).Hash.ToLowerInvariant() -ne $pinned[1]) {
            throw "The gamepad driver is not libvirtualgamepad 0.1.0-beta.10: $($pinned[0])"
        }
    }
}
if (!$PyrowaveRoot) { throw 'Provide the PyroWave SDK path, or use -FetchDependencies' }
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
$env:BUTTERPOLLO_PYROWAVE_ROOT = $PyrowaveRoot
if (!$FfmpegRoot) { throw 'Provide the FFmpeg AC3 build path, or use -FetchDependencies' }
$ffmpegInfo = Join-Path $FfmpegRoot 'share\licenses\ffmpeg\build-info.txt'
if (!(Test-Path -LiteralPath $ffmpegInfo)) { throw 'FFmpeg build-info.txt is missing; rebuild using -FetchDependencies' }
$ffmpegIdentity = Get-Content -LiteralPath $ffmpegInfo -Raw
foreach ($identity in @(
    'source_sha256=7138d28c96d9d3e3af4ee3d8cad72741f8ffb40da90c1112235dea3ecd3178a3',
    'license=LGPL version 2.1 or later'
)) {
    if ($ffmpegIdentity -notmatch ('(?m)^' + [regex]::Escape($identity) + '\r?$')) {
        throw "FFmpeg does not match the pinned LGPL build: expected $identity. Rebuild using -FetchDependencies without -FfmpegRoot."
    }
}
$ffmpegDlls = @('avcodec-62.dll', 'avutil-60.dll')
# The ignored FFmpeg tests decode what the host encodes; the test step below runs them.
$env:BUTTERPOLLO_TEST_FFMPEG_ROOT = Join-Path $FfmpegRoot 'bin'
Push-Location -LiteralPath $repo
try {
    # Workspace members only: --all would also check client/ (the protocol path
    # dependency's whole workspace), which keeps its own formatting.
    & cargo $toolchain fmt -- --check
    Assert-NativeExit 'Rust formatting'
    # The service is a workspace of its own (rust/service).
    $serviceManifest = Join-Path $repo 'rust\service\Cargo.toml'
    & cargo $toolchain fmt --manifest-path $serviceManifest -- --check
    Assert-NativeExit 'Service formatting'
    if ($runTests) {
        & (Join-Path $repo 'rust\drivers\usbip\test_usbip.ps1')
        & cargo $toolchain test --workspace --locked @featureArgs
        Assert-NativeExit 'Rust tests'
        & cargo $toolchain test --manifest-path $serviceManifest --locked
        Assert-NativeExit 'Service tests'
        # The ignored FFmpeg AC3 and ANNOUNCE tests. --workspace like the run
        # above reuses its build: -p would unify features differently and
        # rebuild the workspace crates. The encode-cost benchmark also needs
        # libopus and is run by hand.
        & cargo $toolchain test --workspace --locked @featureArgs -- --ignored ffmpeg:: audio_encoder::tests::ffmpeg_ --skip passthrough_encode_cost
        Assert-NativeExit 'FFmpeg tests'
    }
    if ($runLint) {
        & cargo $toolchain clippy --workspace --all-targets --locked @featureArgs -- -D warnings
        Assert-NativeExit 'Rust lint'
        & cargo $toolchain clippy --manifest-path $serviceManifest --all-targets --locked -- -D warnings
        Assert-NativeExit 'Service lint'
    }
    if (!$runBuild) { return }
    $buildArgs = @('build', '--workspace', '--locked') + $featureArgs
    $profile = 'debug'
    if (!$DebugBuild) { $buildArgs += '--release'; $profile = 'release' }
    & cargo $toolchain @buildArgs
    Assert-NativeExit 'Rust host build'
    $output = Join-Path $TargetDirectory $profile
    # The service's own script makes the same file from the same inputs, in a
    # clean environment and its own target folder, debug packages included.
    & (Get-Process -Id $PID).Path -NoProfile -ExecutionPolicy Bypass -File (Join-Path $repo 'rust\service\build.ps1') -TargetDirectory (Join-Path $TargetDirectory 'service')
    Assert-NativeExit 'Service build'
    Copy-Item -LiteralPath (Join-Path $TargetDirectory 'service\release\butterpollo-service.exe') -Destination $output
    $runtimeDlls = @('libopus-0.dll','libpyrowave-shared-1.dll') + $ffmpegDlls
    Copy-Item -LiteralPath (Join-Path "$MsysRoot\ucrt64\bin" 'libopus-0.dll') -Destination $output
    Copy-Item -LiteralPath (Join-Path $PyrowaveRoot 'bin\libpyrowave-shared-1.dll') -Destination $output
    foreach ($dll in $ffmpegDlls) { Copy-Item -LiteralPath (Join-Path $FfmpegRoot "bin\$dll") -Destination $output }
    if ($Nvidia) {
        # The NGX adapter is an MSVC DLL of its own (NVIDIA's static NGX library is MSVC-only);
        # the GNU host loads it at run time.
        $env:NV_RTX_VIDEO_SDK = $NvidiaRoot
        # The static C runtime: a PC without the Visual C++ Redistributable could not load an
        # adapter that needs MSVCP140.dll and VCRUNTIME140.dll.
        $env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS = '-C target-feature=+crt-static'
        if ($MsvcSdk) {
            $sysroot = & rustc $toolchain --print sysroot
            $env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER = Join-Path $sysroot 'lib\rustlib\x86_64-pc-windows-gnu\bin\rust-lld.exe'
            $env:LIB = "$MsvcSdk\crt\lib\x86_64;$MsvcSdk\sdk\lib\um\x86_64;$MsvcSdk\sdk\lib\ucrt\x86_64;" + $env:LIB
        }
        $adapterManifest = Join-Path $repo 'rust\truehdr-runtime\Cargo.toml'
        & cargo $toolchain fmt --manifest-path $adapterManifest -- --check
        Assert-NativeExit 'NVIDIA adapter formatting'
        if (!$SkipTests) {
            & cargo $toolchain clippy --manifest-path $adapterManifest --target x86_64-pc-windows-msvc --locked -- -D warnings
            Assert-NativeExit 'NVIDIA adapter lint'
        }
        & cargo $toolchain build --manifest-path $adapterManifest --target x86_64-pc-windows-msvc --release --locked
        Assert-NativeExit 'NVIDIA adapter build'
        Copy-Item -LiteralPath (Join-Path $TargetDirectory 'x86_64-pc-windows-msvc\release\butterpollo_truehdr.dll') -Destination $output
        foreach ($dll in @('nvngx_truehdr.dll', 'nvngx_vsr.dll')) {
            Copy-Item -LiteralPath (Join-Path $NvidiaRoot "bin\Windows\x64\rel\$dll") -Destination $output
        }
        $runtimeDlls += @('butterpollo_truehdr.dll', 'nvngx_truehdr.dll', 'nvngx_vsr.dll')
    }
    if ($Package) {
        $distribution = Join-Path $TargetDirectory "butterpollo-rust-$flavor$profile"
        if (Test-Path -LiteralPath $distribution) {
            $resolvedDistribution = (Resolve-Path -LiteralPath $distribution).ProviderPath
            if (!$resolvedDistribution.StartsWith($TargetDirectory.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Package cleanup escaped its output directory' }
            Remove-Item -LiteralPath $resolvedDistribution -Recurse -Force
        }
        New-Item -ItemType Directory -Path "$distribution\assets\web", "$distribution\licenses" -Force | Out-Null
        # With the diagnostic probes from rust/probes.
        foreach ($exe in @('butterpollo.exe', 'butterpollo-service.exe', 'butterpollo-setup.exe', 'butterpollo-performance.exe', 'butterpollo-protocol-performance.exe', 'butterpollo-video-packet-performance.exe')) { Copy-Item -LiteralPath (Join-Path $output $exe) -Destination $distribution }
        # The previous release's service, checked against its SHA256SUMS, while
        # the service's version is unchanged: a new windres or CI image cannot
        # change its file. CI fails when it cannot read that release; a local
        # build without network keeps the fresh one.
        $previousService = @((Join-Path $repo 'rust\release\previous_service.py'), '--exe', (Join-Path $distribution 'butterpollo-service.exe'), '--cache', (Join-Path $TargetDirectory 'previous-release'))
        if ($env:GITHUB_ACTIONS -eq 'true') { $previousService += '--required' }
        if ($env:BUTTERPOLLO_RELEASES_JSON) { $previousService += '--releases', $env:BUTTERPOLLO_RELEASES_JSON }
        & python @previousService
        Assert-NativeExit 'Previous release service'
        Copy-Item -LiteralPath (Join-Path $output 'butterpollo-start.exe') -Destination (Join-Path $distribution 'Start Rubylight.exe')
        Copy-Item -LiteralPath (Join-Path $PyrowaveRoot 'share\pyrowave-shared\build-info.txt') -Destination "$distribution\licenses\pyrowave-build-info.txt"
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
        if ($Nvidia) { Copy-Item -LiteralPath (Join-Path $NvidiaRoot 'NVIDIA_RTX_Video_SDK_License.pdf') -Destination "$distribution\licenses" }
        foreach ($header in @('nvEncodeAPI.h', 'dynlink_cuda.h')) {
            $text = [IO.File]::ReadAllText((Join-Path $repo "rust\windows\include\$header"))
            $notice = $text.Substring(0, $text.IndexOf('*/') + 2)
            $notice | Set-Content -LiteralPath "$distribution\licenses\$header.txt" -Encoding utf8
        }
        Copy-Item -LiteralPath (Join-Path $repo 'rust\service.ps1') -Destination $distribution
        Copy-Item -LiteralPath (Join-Path $repo 'Cargo.lock') -Destination "$distribution\licenses"
        if (Test-Path -LiteralPath (Join-Path $PyrowaveRoot 'share\licenses')) { Copy-Item -LiteralPath (Join-Path $PyrowaveRoot 'share\licenses') -Destination "$distribution\licenses\pyrowave" -Recurse -Force }
        Copy-Item -LiteralPath (Join-Path $FfmpegRoot 'share\licenses\ffmpeg') -Destination "$distribution\licenses\ffmpeg" -Recurse -Force
        foreach ($library in @('gcc-libs','libwinpthread','winpthreads','opus')) {
            $source = Join-Path "$MsysRoot\ucrt64\share\licenses" $library
            if (Test-Path -LiteralPath $source) { Copy-Item -LiteralPath $source -Destination "$distribution\licenses\$library" -Recurse -Force }
        }
        if ($DriverRoot) {
            # Setup installs the gamepad driver with Vibepollo's own scripts
            # (2.0.6's take the same -InstallerBestEffort and
            # -AllowLocalTestCertificate as 2.0.0's).
            New-Item -ItemType Directory -Path "$distribution\drivers" -Force | Out-Null
            Copy-Item -LiteralPath $gamepadRoot -Destination "$distribution\drivers\gamepad" -Recurse -Force
            # The Playnite plugin from the same release; the host installs it
            # into Playnite.
            $playnitePlugin = Join-Path (Split-Path -Parent $DriverRoot) 'plugins\playnite\SunshinePlaynite'
            if (Test-Path -LiteralPath $playnitePlugin) {
                New-Item -ItemType Directory -Path "$distribution\plugins\playnite" -Force | Out-Null
                Copy-Item -LiteralPath $playnitePlugin -Destination "$distribution\plugins\playnite\SunshinePlaynite" -Recurse -Force
            }
            @(
                'Virtual gamepad driver: https://github.com/Nonary/libvirtualgamepad v0.1.0-beta.10 (MIT).',
                'Driver catalog, tools and install scripts as released in Vibepollo 2.0.6 (GPL-3.0),',
                'https://github.com/Nonary/Vibepollo, signed by the SignPath Foundation.',
                'Playnite plugin (plugins\playnite): Sunshine Playnite Connector from the Vibepollo 2.0.0 release (GPL-3.0).',
                'nefconc.exe: https://github.com/nefarius/nefcon by Nefarius Software Solutions.'
            ) | Set-Content -LiteralPath "$distribution\licenses\drivers.txt" -Encoding utf8
        }
        # Rubylight's own display driver, unsigned: setup installs it with
        # install-local.ps1, signing the catalog on the user's PC, and removes
        # the libvirtualdisplay driver earlier versions installed itself
        # (rust/setup/src/libvirtualdisplay.rs). A signed release replaces the catalog with SignPath's
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
        $serviceMetadata = & cargo $toolchain metadata --format-version 1 --locked --manifest-path $serviceManifest
        Assert-NativeExit 'Service dependency manifest'
        $packages = @(($metadata | ConvertFrom-Json).packages)
        $known = @{}
        # Not $package: that is this script's -Package switch.
        foreach ($dependency in $packages) { $known["$($dependency.name) $($dependency.version)"] = $true }
        $packages += @(($serviceMetadata | ConvertFrom-Json).packages | Where-Object { !$known["$($_.name) $($_.version)"] })
        $packages | Select-Object name, version, license, repository, source | ConvertTo-Json | Set-Content -LiteralPath "$distribution\licenses\rust-dependencies.json" -Encoding utf8
        $version = (Select-String -LiteralPath (Join-Path $repo 'Cargo.toml') -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
        & (Join-Path $repo 'rust\release\package.ps1') -Distribution $distribution -TargetDirectory $TargetDirectory `
            -Stub (Join-Path $output 'butterpollo-setup.exe') -Version $version -ProfileName $profile -Nvidia:$Nvidia
    }
    Write-Output "Rust executables: $output"
} finally { Pop-Location }
} finally {
    try { Exit-CheckLocks $buildLease -NoPause }
    finally {
        $env:RUBYLIGHT_BUILD_LOCK_OWNER = $previousBuildOwner
        $env:RUBYLIGHT_BUILD_LOCK_PATH = $previousBuildPath
    }
}
