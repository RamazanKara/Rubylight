# Started by the ButterpolloReleaseElevated task (see elevation.ps1) with
# highest privileges. Reads the request check.ps1 wrote, spends it, and runs
# the elevated step for it. Only paths inside release.ps1's default work folder
# are accepted. Also serves the WebRTC end-to-end harness (below).
$ErrorActionPreference = 'Stop'
$root = 'C:\src\butterpollo-release\'
$release = Join-Path $root 'request.json'
$fresh = { param($path) (Get-Item -LiteralPath $path -Force).LastWriteTime -gt (Get-Date).AddMinutes(-2) }
# The WebRTC end-to-end harness asks through its own request, which webrtc-e2e-elevated.ps1 checks
# step by step and spends. One older than two minutes is from a run that died before the task
# started: it is left alone. The folder is the user's, so a link there is refused before anything
# in it is read.
$e2eDir = Join-Path $root 'webrtc-e2e'
$e2e = Join-Path $e2eDir 'request.json'
foreach ($path in $e2eDir, $e2e) {
    $item = Get-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue
    if ($item -and ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw "$path is a link or junction; refusing it" }
}
if ((Test-Path -LiteralPath $e2e) -and (& $fresh $e2e)) {
    & (Join-Path $PSScriptRoot 'webrtc-e2e-elevated.ps1')
    # The task ignores a start while it runs, so a release request that came meanwhile is served
    # now. A release request is spent once read; one still here and fresh has not been served.
    if (-not ((Test-Path -LiteralPath $release) -and (& $fresh $release))) { return }
}
$request = Get-Content -LiteralPath $release -Raw | ConvertFrom-Json
Remove-Item -LiteralPath $release -Force
$version = [string]$request.version
if ($version -notmatch '^\d+\.\d+\.\d+(-(alpha|beta|rc)\.\d+)?$') { throw "invalid version $version" }
$work = [IO.Path]::GetFullPath((Join-Path $root $version))
$package = [IO.Path]::GetFullPath("$work\release\butterpollo-rust-release")
$installer = [IO.Path]::GetFullPath("$work\release\rubylight-setup-$version.exe")
if (-not (Test-Path -LiteralPath $installer)) {
    # Releases before rc.30 carry only the Butterpollo name.
    $installer = [IO.Path]::GetFullPath("$work\release\butterpollo-setup-$version.exe")
}
foreach ($path in $work, $package, $installer) {
    if (-not $path.StartsWith($root, [StringComparison]::OrdinalIgnoreCase)) { throw "$path is outside $root" }
}
& (Join-Path $PSScriptRoot 'elevated.ps1') -Package $package -Installer $installer -Version $version -Work $work
