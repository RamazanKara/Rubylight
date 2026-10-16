# Dot-source before building the driver: . .\driver-env.ps1
# The MSVC developer environment (link.exe, the CRT and SDK paths bindgen needs), LLVM's
# libclang for wdk-sys' bindgen, and rustup's cargo. Deliberately not the host's
# C:\src\rust-env.ps1: that one puts MSYS2's GNU tools and libclang first.
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$vs = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $vs) { throw 'Visual Studio C++ build tools (x64) are required' }
$env:PATH = "$(Split-Path $vswhere);" + $env:PATH  # VsDevCmd looks vswhere up on PATH
Import-Module "$vs\Common7\Tools\Microsoft.VisualStudio.DevShell.dll"
Enter-VsDevShell -VsInstallPath $vs -SkipAutomaticLocation -DevCmdArguments '-arch=x64 -host_arch=x64' | Out-Null
$llvm = @("$env:ProgramFiles\LLVM\bin", "${env:ProgramFiles(x86)}\LLVM\bin") | Where-Object { Test-Path "$_\libclang.dll" } | Select-Object -First 1
if (-not $llvm) { throw 'LLVM (libclang.dll) is required: winget install LLVM.LLVM' }
$env:LIBCLANG_PATH = $llvm
$env:PATH = "$env:USERPROFILE\.cargo\bin;" + $env:PATH
# rust\build.ps1 points CARGO_TARGET_DIR at the host's target folder. The driver keeps its own:
# build-package.ps1 reads target\ here, and wdk-build finds the workspace's WDK metadata from it.
Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
# IddCx 1.11 (the D3D12 swap chain) is newer than the installed kit; its headers and stub come
# from the pinned WDK NuGet package.
. "$PSScriptRoot\fetch-iddcx.ps1"
$env:RLVD_IDDCX_ROOT = $iddcxRoot
