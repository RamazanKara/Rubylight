# Builds a release nightjar.exe and zips it with the DLLs it needs, so it runs on a PC
# without MSYS2.
#
#   .\package-windows.ps1 [-FfmpegDir <prefix from ffmpeg-decode-only.sh>] [-Out <folder>] [-TargetDirectory <dir>]
#
# Needs the toolchain from README.md ("Building", Windows). The DLLs are found by reading the
# import tables (objdump) of the exe and of every DLL copied, down to the system ones, which are
# left out. With -FfmpegDir the exe is built against that decode-only FFmpeg and its DLLs are
# preferred; without it the package carries MSYS2's FFmpeg and everything that drags in.
param(
    [string]$FfmpegDir = '',
    [string]$Out = (Join-Path $PSScriptRoot '..\..\dist'),
    [string]$TargetDirectory = ''
)
$ErrorActionPreference = 'Stop'

$msys = 'C:\msys64\ucrt64\bin'
$workspace = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$env:PATH = "$msys;$env:USERPROFILE\.cargo\bin;" + $env:PATH
if (-not $env:LIBCLANG_PATH) { $env:LIBCLANG_PATH = $msys }
$search = @($msys)
if ($FfmpegDir) {
    $FfmpegDir = (Resolve-Path $FfmpegDir).Path
    $env:NIGHTJAR_FFMPEG_DIR = $FfmpegDir
    $search = @("$FfmpegDir\bin") + $search
}
if (-not $TargetDirectory) { $TargetDirectory = Join-Path $workspace 'target' }

Push-Location $workspace
try {
    cargo build --release -p nightjar-desktop --locked --target-dir $TargetDirectory
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
} finally { Pop-Location }
$exe = Join-Path $TargetDirectory 'release\nightjar.exe'

# What a DLL or exe imports, by name.
function Get-Imports([string]$path) {
    & "$msys\objdump.exe" -p $path | ForEach-Object {
        if ($_ -match 'DLL Name:\s*(\S+)') { $Matches[1] }
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
while ($queue.Count -gt 0) {
    foreach ($name in Get-Imports $queue.Dequeue()) {
        if ($found.Contains($name.ToLowerInvariant()) -or (Test-System $name)) { continue }
        $path = $search | ForEach-Object { Join-Path $_ $name } | Where-Object { Test-Path $_ } | Select-Object -First 1
        if (-not $path) { throw "$name is imported but is neither a system library nor in $($search -join ', ')" }
        $found[$name.ToLowerInvariant()] = $path
        $queue.Enqueue($path)
    }
}

$version = (Select-String -Path (Join-Path $workspace 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches.Groups[1].Value
$name = "nightjar-desktop-$version-windows-x64"
$stage = Join-Path $Out $name
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item $exe $stage
foreach ($path in $found.Values) { Copy-Item $path $stage }
Copy-Item (Join-Path $PSScriptRoot 'README.md') $stage
$zip = Join-Path $Out "$name.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path "$stage\*" -DestinationPath $zip -CompressionLevel Optimal

$bytes = (Get-ChildItem $stage -File | Measure-Object Length -Sum).Sum
Get-ChildItem $stage -File | Sort-Object Length -Descending | ForEach-Object { '{0,12:N0}  {1}' -f $_.Length, $_.Name }
'{0} files, {1:N1} MB unpacked, {2:N1} MB zipped: {3}' -f (Get-ChildItem $stage -File).Count, ($bytes / 1MB), ((Get-Item $zip).Length / 1MB), $zip
