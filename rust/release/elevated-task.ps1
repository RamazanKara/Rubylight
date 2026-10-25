# Started by the ButterpolloReleaseElevated task (see elevation.ps1) with
# highest privileges. Reads the request release.ps1 wrote and runs the
# elevated step for it. Only paths inside release.ps1's default work folder
# are accepted. Also serves the WebRTC end-to-end harness (below).
$ErrorActionPreference = 'Stop'
$root = 'C:\src\butterpollo-release\'
# The WebRTC end-to-end harness asks through its own request, which webrtc-e2e-elevated.ps1 checks
# step by step and spends. One older than two minutes is from a run that died before the task
# started; it is dropped, and the release request is served.
$e2e = Join-Path $root 'webrtc-e2e\request.json'
if (Test-Path -LiteralPath $e2e) {
    if ((Get-Item -LiteralPath $e2e).LastWriteTime -gt (Get-Date).AddMinutes(-2)) {
        & (Join-Path $PSScriptRoot 'webrtc-e2e-elevated.ps1')
        return
    }
    Remove-Item -LiteralPath $e2e -Force
}
$request = Get-Content (Join-Path $root 'request.json') -Raw | ConvertFrom-Json
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
