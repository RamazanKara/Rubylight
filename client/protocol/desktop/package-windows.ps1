# Builds a release nightjar.exe and zips it with the DLLs it needs, so it runs on a PC
# without MSYS2.
#
#   .\package-windows.ps1 [-FfmpegDir <prefix from ffmpeg-decode-only.sh>] [-PyrowaveSdkDir <folder from
#       scripts\build-pyrowave-sdk.ps1>] [-Out <folder>] [-TargetDirectory <dir>] [-Version <version>]
#       [-Msys <MSYS2 folder>] [-Target <Rust target>]
#
# Needs the toolchain from README.md ("Building", Windows): on an x86_64 PC the GNU Rust toolchain
# and MSYS2 UCRT64, on an ARM64 PC the gnullvm one and MSYS2 CLANGARM64 (-Msys, default C:\msys64,
# names the MSYS2 installation). -Target builds for another architecture than this PC's
# (aarch64-pc-windows-gnullvm on an x86_64 PC, with an aarch64 cross compiler and CLANGARM64's
# libraries set up by the caller); such an exe cannot start here, so the start checks are skipped
# and the result is for inspection, never for release. The DLLs are found by reading the
# import tables (objdump) of the exe and of every DLL copied, down to the system ones, which are
# left out. With -FfmpegDir the exe is built against that decode-only FFmpeg and its DLLs are
# preferred; without it the package carries MSYS2's FFmpeg and everything that drags in.
#
# With -PyrowaveSdkDir the package also carries the PyroWave SDK the GPU decoder loads at run time
# (libpyrowave-shared-1.dll, so the import walk cannot see it): the DLL next to the exe, its
# build-info.txt and its licences. The script refuses a DLL that is not the one protocol\desktop\
# pyrowave-sdk.pin names (commit, Granite, API, patches) or whose SHA-256 is not the one the build
# recorded. Without it the package has no GPU decoder and the client says so when asked for it.
# With the SDK the unpacked exe is also run as `nightjar.exe --check-pyrowave <file>` (with only the
# Windows folders on PATH): the package must find and load the DLL (always checked) and, with
# -RequireGpuProbe, build a decoder on this PC's GPU (CI runners have none).
# -Version names the package after a release version instead of the crate's (the release workflow
# passes the app version): nightjar-desktop-<version>-windows-<x86_64 or arm64>.zip.
param(
    [string]$FfmpegDir = '',
    [string]$PyrowaveSdkDir = '',
    [string]$Out = (Join-Path $PSScriptRoot '..\..\dist'),
    [string]$TargetDirectory = '',
    [string]$Version = '',
    [string]$Msys = 'C:\msys64',
    [ValidateSet('', 'x86_64-pc-windows-gnu', 'aarch64-pc-windows-gnullvm')]
    [string]$Target = '',
    [switch]$RequireGpuProbe
)
$ErrorActionPreference = 'Stop'

# The package is named after Windows' own word for each architecture (x86_64 as before, arm64).
$native = if ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -eq 'Arm64') { 'arm64' } else { 'x86_64' }
$arch = switch -Wildcard ($Target) { '' { $native } 'aarch64-*' { 'arm64' } default { 'x86_64' } }
$runs = $arch -eq $native -or $native -eq 'arm64'
# $msys holds the target's DLLs and, in a native build, the tools. A cross build leaves PATH and
# LIBCLANG_PATH to its caller: the other architecture's MSYS2 programs may not run on this PC.
$msys = Join-Path $Msys $(if ($arch -eq 'arm64') { 'clangarm64\bin' } else { 'ucrt64\bin' })
$cross = $arch -ne $native
$workspace = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
if (-not $cross) {
    $env:PATH = "$msys;$env:USERPROFILE\.cargo\bin;" + $env:PATH
    if (-not $env:LIBCLANG_PATH) { $env:LIBCLANG_PATH = $msys }
}
$search = @($msys)
if ($FfmpegDir) {
    $FfmpegDir = (Resolve-Path $FfmpegDir).Path
    $env:NIGHTJAR_FFMPEG_DIR = $FfmpegDir
    $search = @("$FfmpegDir\bin") + $search
}
if (-not $TargetDirectory) { $TargetDirectory = Join-Path $workspace 'target' }

# key=value lines (the pin, build-info.txt) as a hashtable.
function Read-KeyValues([string]$path) {
    $values = @{}
    Get-Content $path | Where-Object { $_ -match '^\s*[a-z_0-9]+=' } | ForEach-Object {
        $key, $value = $_ -split '=', 2
        $values[$key.Trim()] = $value.Trim()
    }
    $values
}
# The SDK is checked before the long build, so a wrong one fails in a second.
$sdkDll = $null
if ($PyrowaveSdkDir) {
    $PyrowaveSdkDir = (Resolve-Path $PyrowaveSdkDir).Path
    $sdkDll = Join-Path $PyrowaveSdkDir 'libpyrowave-shared-1.dll'
    foreach ($path in $sdkDll, (Join-Path $PyrowaveSdkDir 'build-info.txt'), (Join-Path $PyrowaveSdkDir 'licenses')) {
        if (-not (Test-Path $path)) { throw "$path is missing; build the SDK with scripts\build-pyrowave-sdk.ps1" }
    }
    $pin = Read-KeyValues (Join-Path $PSScriptRoot 'pyrowave-sdk.pin')
    $built = Read-KeyValues (Join-Path $PyrowaveSdkDir 'build-info.txt')
    foreach ($key in 'pyrowave_commit', 'api_version', 'patches') {
        if ($built[$key] -ne $pin[$key]) { throw "the PyroWave SDK in $PyrowaveSdkDir has $key=$($built[$key]), pyrowave-sdk.pin says $($pin[$key])" }
    }
    if (-not "$($built['granite_commit'])".StartsWith($pin['granite_commit'])) {
        throw "the PyroWave SDK in $PyrowaveSdkDir has granite_commit=$($built['granite_commit']), pyrowave-sdk.pin says $($pin['granite_commit'])"
    }
    $sha = (Get-FileHash $sdkDll -Algorithm SHA256).Hash.ToLowerInvariant()
    $notice = Join-Path $PyrowaveSdkDir 'licenses\LICENSE.winpthreads'
    if (-not (Test-Path $notice)) { throw "$notice is missing: the DLL carries winpthreads and its notice must ship; rebuild the SDK with scripts\build-pyrowave-sdk.ps1" }
    if ($built['dll_sha256'] -ne $sha) { throw "libpyrowave-shared-1.dll is $sha, build-info.txt recorded $($built['dll_sha256']): it is not the DLL that was built" }
}

Push-Location $workspace
try {
    $targetArgs = if ($Target) { @('--target', $Target) } else { @() }
    cargo build --release -p nightjar-desktop --locked --target-dir $TargetDirectory @targetArgs
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
} finally { Pop-Location }
$exe = Join-Path $TargetDirectory $(if ($Target) { "$Target\release\nightjar.exe" } else { 'release\nightjar.exe' })
# An x86_64 exe or DLL must not end up in the arm64 package or the other way round: the PE header's
# machine field of each one (the exe here, every shipped file before zipping).
function Get-Machine([string]$path) {
    $stream = [IO.File]::OpenRead($path)
    try {
        $reader = [IO.BinaryReader]::new($stream)
        $stream.Position = 0x3C
        $stream.Position = $reader.ReadInt32() + 4
        $reader.ReadUInt16()
    } finally { $stream.Dispose() }
}
$expectedMachine = if ($arch -eq 'arm64') { 0xAA64 } else { 0x8664 }
function Assert-Machine([string]$path) {
    $machine = Get-Machine $path
    if ($machine -ne $expectedMachine) { throw ('{0} is built for machine 0x{1:X4}, not {2}' -f $path, $machine, $arch) }
}
Assert-Machine $exe

# build.rs embeds packaging/icons/nightjar.ico; Explorer, the taskbar and the window all take it from here.
if (-not ('NightjarPackage.Shell' -as [type])) {
    Add-Type -Namespace NightjarPackage -Name Shell -MemberDefinition '[DllImport("shell32.dll", CharSet = CharSet.Unicode)] public static extern uint ExtractIconEx(string file, int index, IntPtr[] large, IntPtr[] small, uint icons);'
}
if ([NightjarPackage.Shell]::ExtractIconEx($exe, -1, $null, $null, 0) -lt 1) { throw "$exe has no icon resource" }

# What a DLL or exe imports, by name. MSYS2's objdump is GNU binutils on UCRT64 and LLVM's on
# CLANGARM64; a cross build takes llvm-objdump from its caller's PATH (it reads both machines).
# Both print the import table's "DLL Name:" lines; LLVM's also prints a DLL's own name from its
# export table as "DLL name:", which is no import, so the match keeps to the capital.
$objdump = if ($cross) { (Get-Command llvm-objdump.exe -ErrorAction Stop).Source } else { "$msys\objdump.exe" }
function Get-Imports([string]$path) {
    & $objdump -p $path | ForEach-Object {
        if ($_ -cmatch 'DLL Name:\s*(\S+)') { $Matches[1] }
    }
}
# Windows' own libraries: api sets, and anything System32 has that MSYS2 does not rebuild.
function Test-System([string]$name) {
    ($name -like 'api-ms-win-*') -or ($name -like 'ext-ms-win-*') -or
        ((Test-Path (Join-Path $env:WINDIR "System32\$name")) -and -not (Test-Path (Join-Path $msys $name)))
}

$found = [ordered]@{}
$queue = [System.Collections.Generic.Queue[string]]::new()
$queue.Enqueue($exe)
# The SDK is loaded at run time, so nothing imports it; its own imports still need to travel.
if ($sdkDll) { $queue.Enqueue($sdkDll) }
while ($queue.Count -gt 0) {
    foreach ($name in Get-Imports $queue.Dequeue()) {
        if ($found.Contains($name.ToLowerInvariant()) -or (Test-System $name)) { continue }
        $path = $search | ForEach-Object { Join-Path $_ $name } | Where-Object { Test-Path $_ } | Select-Object -First 1
        if (-not $path) { throw "$name is imported but is neither a system library nor in $($search -join ', ')" }
        $found[$name.ToLowerInvariant()] = $path
        $queue.Enqueue($path)
    }
}

$version = $Version
if (-not $version) { $version = (Select-String -Path (Join-Path $workspace 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches.Groups[1].Value }
$name = "nightjar-desktop-$version-windows-$arch"
$stage = Join-Path $Out $name
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item $exe $stage
foreach ($path in $found.Values) { Copy-Item $path $stage }
# MSYS2 runtime DLLs whose licence must travel with them: the ARM64 exe imports LLVM's
# libunwind.dll (Rust's gnullvm target links it dynamically), under Apache-2.0 with LLVM
# exceptions, whose exception covers only code built into a program, not the DLL itself.
$llvmRuntime = @{ 'libunwind.dll' = 'libunwind'; 'libc++.dll' = 'libc++' }
foreach ($dll in $llvmRuntime.Keys | Where-Object { $found.Contains($_) }) {
    $licence = Join-Path (Split-Path $msys) "share\licenses\$($llvmRuntime[$dll])\LICENSE"
    if (-not (Test-Path $licence)) { throw "$dll ships, but its licence $licence is missing; install the MSYS2 package that owns it" }
    New-Item -ItemType Directory -Force (Join-Path $stage 'licenses-llvm') | Out-Null
    Copy-Item $licence (Join-Path $stage "licenses-llvm\LICENSE.$($llvmRuntime[$dll])")
}
if ($sdkDll) {
    Copy-Item $sdkDll $stage
    Copy-Item (Join-Path $PyrowaveSdkDir 'build-info.txt') (Join-Path $stage 'pyrowave-sdk-build-info.txt')
    Copy-Item (Join-Path $PyrowaveSdkDir 'licenses') (Join-Path $stage 'licenses-pyrowave') -Recurse
}
Copy-Item (Join-Path $PSScriptRoot 'README.md') $stage
# sonora's BSD notice: the microphone's noise suppression and automatic gain are built in.
New-Item -ItemType Directory -Force (Join-Path $stage 'licenses-sonora') | Out-Null
Copy-Item (Join-Path $PSScriptRoot 'assets\NOTICE-SONORA.txt') (Join-Path $stage 'licenses-sonora')
# The decode-only FFmpeg's (LGPL, with the source offer) and dav1d's (BSD) licences, from the prefix.
if ($FfmpegDir) {
    foreach ($licences in 'licenses-ffmpeg', 'licenses-dav1d') {
        $from = Join-Path $FfmpegDir $licences
        if (-not (Test-Path $from)) { throw "$from is missing; build the prefix with ffmpeg-decode-only.sh" }
        Copy-Item $from (Join-Path $stage $licences) -Recurse
    }
}

# The shipped files must run on a clean Windows 10/11: every import of every DLL and exe in the
# package is a file in the package or a library Windows itself ships. The Visual C++ runtime is
# not one (a clean PC does not have it), even when this machine does.
$shipped = @{}
Get-ChildItem $stage -File | ForEach-Object { $shipped[$_.Name.ToLowerInvariant()] = $true }
$missing = foreach ($file in Get-ChildItem $stage -File | Where-Object { $_.Extension -in '.exe', '.dll' }) {
    Assert-Machine $file.FullName
    foreach ($import in Get-Imports $file.FullName) {
        if ($shipped.ContainsKey($import.ToLowerInvariant())) { continue }
        if ((Test-System $import) -and ($import -notmatch '^(vcruntime|msvcp|vcomp|concrt|msvcr|ucrtbased|vccorlib)')) { continue }
        "$($file.Name) imports $import"
    }
}
if ($missing) { throw "the package would not start on a clean Windows PC:`n$($missing -join "`n")" }
$zip = Join-Path $Out "$name.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path "$stage\*" -DestinationPath $zip -CompressionLevel Optimal

if (-not $runs) {
    Write-Warning "an $arch exe cannot start on this $native PC: the start checks are skipped, and $zip is for inspection only"
    return
}

# Start the unpacked zip with only Windows' own folders on PATH (so a DLL it forgot cannot come
# from MSYS2) and an empty config folder. The exe has no command line, so it opens its window;
# a missing DLL or entry point ends the process at once with 0xC0000135 or 0xC0000139, anything
# still running after a few seconds started. The folder keeps one name: Windows' firewall asks
# about each program path that opens a socket, so a new folder per run would ask every time.
$unpacked = Join-Path ([IO.Path]::GetTempPath()) 'nightjar-smoke'
if (Test-Path $unpacked) { Remove-Item -Recurse -Force $unpacked }
Expand-Archive -Path $zip -DestinationPath $unpacked
$savedPath = $env:PATH
$env:PATH = "$env:WINDIR\System32;$env:WINDIR"
$env:NIGHTJAR_CONFIG_DIR = Join-Path $unpacked 'config'
try {
    $process = Start-Process -FilePath (Join-Path $unpacked 'nightjar.exe') -WorkingDirectory $unpacked -PassThru
    if ($process.WaitForExit(6000)) {
        if ($process.ExitCode -in -1073741515, -1073741511) { throw ('nightjar.exe from the zip does not start: exit code 0x{0:X8} (a DLL or an export is missing)' -f $process.ExitCode) }
        if ($process.ExitCode -ne 0) { throw ('nightjar.exe from the zip exited with {0} (0x{0:X8}) within 6 s' -f $process.ExitCode) }
        Write-Warning 'nightjar.exe from the zip exited with 0 within 6 s'
    } else {
        Write-Host 'smoke: nightjar.exe from the zip is still running after 6 s'
        $process.Kill()
        $process.WaitForExit()
    }
    if ($sdkDll) {
        $report = Join-Path $unpacked 'pyrowave-check.txt'
        $check = Start-Process -FilePath (Join-Path $unpacked 'nightjar.exe') -ArgumentList '--check-pyrowave', $report -WorkingDirectory $unpacked -Wait -PassThru
        $text = if (Test-Path $report) { (Get-Content $report) -join '; ' } else { 'no report written' }
        Write-Host "pyrowave check (exit $($check.ExitCode)): $text"
        if ($check.ExitCode -notin 0, 2 -or ($RequireGpuProbe -and $check.ExitCode -ne 0)) {
            throw "nightjar.exe from the zip cannot use the PyroWave SDK it carries (exit $($check.ExitCode)): $text"
        }
    }
} finally {
    $env:PATH = $savedPath
    Remove-Item Env:NIGHTJAR_CONFIG_DIR -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $unpacked -ErrorAction SilentlyContinue
}

$bytes = (Get-ChildItem $stage -File | Measure-Object Length -Sum).Sum
Get-ChildItem $stage -File | Sort-Object Length -Descending | ForEach-Object { '{0,12:N0}  {1}' -f $_.Length, $_.Name }
'{0} files, {1:N1} MB unpacked, {2:N1} MB zipped: {3}' -f (Get-ChildItem $stage -File).Count, ($bytes / 1MB), ((Get-Item $zip).Length / 1MB), $zip
