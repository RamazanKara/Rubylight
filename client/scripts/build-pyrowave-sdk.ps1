# Builds the PyroWave SDK the desktop client's GPU decoder loads, at the pin in
# protocol/desktop/pyrowave-sdk.pin with its patches, into a folder outside the repo:
#
#   .\scripts\build-pyrowave-sdk.ps1 [-Out <folder>] [-HostRepo <Rubylight checkout>] [-Jobs <n>]
#
# <Out> gets libpyrowave-shared-1.dll, build-info.txt (the pin, the patches, the DLL's SHA-256 and
# the toolchain) and licenses\. The DLL is built by the Rubylight host's rust/tools/build_pyrowave.sh
# (MSYS2 UCRT64 with git, gcc, cmake 3.27 or newer and ninja), which is copied next to the patch set
# so it applies 0002 (from the host's rust/codec-patches/pyrowave) and 0003 to 0005 (from this
# repo's app/src/main/jni/pyrowave-renderer/patches). The sources and the build tree stay in
# <Out>\work, so a second run only rebuilds what changed.
# package-windows.ps1 reads <Out> through -PyrowaveSdkDir.
param(
    [string]$Out = 'C:\src\butterpollo-release\nj-pyrowave-sdk',
    [string]$HostRepo = 'C:\src\rubylight-rel',
    [int]$Jobs = 0
)
$ErrorActionPreference = 'Stop'

$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$msys = 'C:\msys64'
$pin = @{}
Get-Content (Join-Path $repo 'protocol\desktop\pyrowave-sdk.pin') | Where-Object { $_ -match '^\s*[a-z_]+=' } | ForEach-Object {
    $key, $value = $_ -split '=', 2
    $pin[$key.Trim()] = $value.Trim()
}
foreach ($key in 'pyrowave_commit', 'granite_commit', 'api_version', 'patches') {
    if (-not $pin[$key]) { throw "pyrowave-sdk.pin has no $key" }
}

$hostScript = Join-Path $HostRepo 'rust\tools\build_pyrowave.sh'
$hostPatches = Join-Path $HostRepo 'rust\codec-patches\pyrowave'
$ownPatches = Join-Path $repo 'app\src\main\jni\pyrowave-renderer\patches'
foreach ($path in $hostScript, $hostPatches, $ownPatches, "$msys\usr\bin\bash.exe") {
    if (-not (Test-Path $path)) { throw "$path is missing" }
}

New-Item -ItemType Directory -Force $Out | Out-Null
$Out = (Resolve-Path $Out).Path
$work = Join-Path $Out 'work'
$stage = Join-Path $work 'stage'
$prefix = Join-Path $work 'install'
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
if (Test-Path $prefix) { Remove-Item -Recurse -Force $prefix }
New-Item -ItemType Directory -Force "$stage\tools", "$stage\codec-patches\pyrowave" | Out-Null
Copy-Item $hostScript "$stage\tools\build_pyrowave.sh"

# The patch set, in the pin's order. A patch both repos carry must be the same file.
foreach ($name in $pin['patches'] -split ',') {
    $candidates = @(@(Join-Path $hostPatches "$name.patch"; Join-Path $ownPatches "$name.patch") | Where-Object { Test-Path $_ })
    if (-not $candidates) { throw "patch $name is in neither $hostPatches nor $ownPatches" }
    if (@($candidates | ForEach-Object { (Get-FileHash $_).Hash } | Select-Object -Unique).Count -ne 1) {
        throw "patch $name differs between $hostPatches and $ownPatches"
    }
    Copy-Item $candidates[0] "$stage\codec-patches\pyrowave\$name.patch"
}
# Any other patch in the host's folder would be applied too: the stage holds the pin's patches only.

function ConvertTo-Msys([string]$path) { '/' + $path.Substring(0, 1).ToLowerInvariant() + $path.Substring(2).Replace('\', '/') }
$env:MSYSTEM = 'UCRT64'
$env:CHERE_INVOKING = '1'
$env:PYROWAVE_WORKDIR = ConvertTo-Msys $work
if ($Jobs -gt 0) { $env:PYROWAVE_JOBS = "$Jobs" }
$command = 'bash {0} {1} {2}' -f (ConvertTo-Msys "$stage\tools\build_pyrowave.sh"), $pin['pyrowave_commit'], (ConvertTo-Msys $prefix)
& "$msys\usr\bin\bash.exe" -lc $command
if ($LASTEXITCODE -ne 0) { throw "build_pyrowave.sh failed with exit code $LASTEXITCODE" }

$dll = Join-Path $prefix 'bin\libpyrowave-shared-1.dll'
$info = Join-Path $prefix 'share\pyrowave-shared\build-info.txt'
foreach ($path in $dll, $info) { if (-not (Test-Path $path)) { throw "the build did not produce $path" } }
$built = @{}
Get-Content $info | ForEach-Object { $key, $value = $_ -split '=', 2; $built[$key] = $value }
if ($built['pyrowave_commit'] -ne $pin['pyrowave_commit']) { throw "built $($built['pyrowave_commit']), the pin is $($pin['pyrowave_commit'])" }
if (-not $built['granite_commit'].StartsWith($pin['granite_commit'])) { throw "built Granite $($built['granite_commit']), the pin is $($pin['granite_commit'])" }
if ($built['api_version'] -ne $pin['api_version']) { throw "built API $($built['api_version']), the pin is $($pin['api_version'])" }
if ($built['patches'] -ne $pin['patches']) { throw "built with patches $($built['patches']), the pin is $($pin['patches'])" }

Copy-Item $dll $Out -Force
if (Test-Path "$Out\licenses") { Remove-Item -Recurse -Force "$Out\licenses" }
Copy-Item (Join-Path $prefix 'share\licenses\pyrowave') "$Out\licenses" -Recurse
# The DLL carries the MinGW runtime statically (it imports no libwinpthread or libstdc++ DLL), and
# the licences of those libraries ask for their notices to travel with it.
$runtime = @{
    'LICENSE.winpthreads'            = @('ucrt64\share\licenses\winpthreads\COPYING', 'ucrt64\share\licenses\libwinpthread\COPYING')
    # gcc 16.2 split gcc-libs into libgcc, libstdc++ and others; older MSYS2 still has gcc-libs.
    'LICENSE.gcc-runtime-exception'  = @('ucrt64\share\licenses\libgcc\COPYING.RUNTIME', 'ucrt64\share\licenses\gcc-libs\COPYING.RUNTIME', 'ucrt64\share\licenses\gcc\COPYING.RUNTIME')
}
foreach ($name in $runtime.Keys) {
    $candidates = @($runtime[$name] | ForEach-Object { Join-Path $msys $_ })
    $source = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
    if (-not $source) { throw "none of $($candidates -join ', ') exists; install the MSYS2 package that owns it (mingw-w64-ucrt-x86_64-winpthreads, mingw-w64-ucrt-x86_64-libgcc or -gcc-libs)" }
    Copy-Item $source "$Out\licenses\$name"
}
$sha = (Get-FileHash (Join-Path $Out 'libpyrowave-shared-1.dll') -Algorithm SHA256).Hash.ToLowerInvariant()
$text = (Get-Content $info) + "dll_sha256=$sha" + "built=$([DateTime]::UtcNow.ToString('yyyy-MM-ddTHH:mm:ssZ'))"
[System.IO.File]::WriteAllText((Join-Path $Out 'build-info.txt'), (($text -join "`n") + "`n"))
"PyroWave SDK $($pin['pyrowave_commit']) with $($pin['patches']) built into $Out"
$text
