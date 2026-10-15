param(
    [Parameter(Mandatory)][ValidateSet('CheckMetadata', 'StageBinaries', 'ApplyBinaries', 'ApplyInstaller')][string]$Phase,
    [Parameter(Mandatory)][string]$TargetDirectory,
    [string]$SigningDirectory,
    [Parameter(Mandatory)][string]$Version
)
$ErrorActionPreference = 'Stop'
$distribution = Join-Path $TargetDirectory 'butterpollo-rust-release'
# Rubylight's own binaries, signed by the "binaries" request. Keep this list
# and .signpath/binaries.xml in step; rust/release/test_release.py checks.
$binaries = @(
    'butterpollo.exe', 'butterpollo-service.exe', 'Start Rubylight.exe',
    'butterpollo-setup.exe', 'butterpollo-performance.exe',
    'butterpollo-protocol-performance.exe', 'butterpollo-video-packet-performance.exe',
    'butterpollo_truehdr.dll', 'vulkan-layer/butterpollo_vulkan_layer.dll'
)
function Get-Release {
    # The first-party binaries this package ships, relative to the distribution.
    $names = @($binaries)
    $names
}
function Assert-Signed([string]$Path) {
    $signature = Get-AuthenticodeSignature -LiteralPath $Path
    if ($signature.Status -ne 'Valid' -or !$signature.SignerCertificate -or !$signature.TimeStamperCertificate) {
        throw "Missing valid timestamped Authenticode signature: $Path ($($signature.Status))"
    }
}
function Assert-Metadata([string]$Path) {
    # SignPath signs only files whose version information names the project
    # and the release: the restrictions in .signpath/*.xml.
    $info = (Get-Item -LiteralPath $Path).VersionInfo
    if ($info.ProductName -ne 'Rubylight' -or $info.ProductVersion -ne $Version) {
        throw "$Path has product '$($info.ProductName)' version '$($info.ProductVersion)', not Rubylight $Version"
    }
}
if ($Phase -ne 'CheckMetadata' -and !$SigningDirectory) { throw "-SigningDirectory is required for $Phase" }
if ($Phase -eq 'CheckMetadata') {
    foreach ($name in Get-Release) { Assert-Metadata (Join-Path $distribution $name) }
    $installer = Join-Path $TargetDirectory "butterpollo-setup-$Version.exe"
    if (Test-Path -LiteralPath $installer) { Assert-Metadata $installer }
} elseif ($Phase -eq 'StageBinaries') {
    foreach ($name in Get-Release) {
        $source = Join-Path $distribution $name
        if (!(Test-Path -LiteralPath $source -PathType Leaf)) { throw "Missing release binary: $name" }
        $target = Join-Path $SigningDirectory $name
        New-Item -ItemType Directory -Path (Split-Path -Parent $target) -Force | Out-Null
        Copy-Item -LiteralPath $source -Destination $target
    }
} elseif ($Phase -eq 'ApplyBinaries') {
    # Verify the complete set before replacing anything. Third-party binaries
    # retain their upstream signatures and are never submitted for re-signing.
    $names = Get-Release
    foreach ($name in $names) { Assert-Signed (Join-Path $SigningDirectory $name) }
    foreach ($name in $names) {
        Copy-Item -LiteralPath (Join-Path $SigningDirectory $name) -Destination (Join-Path $distribution $name) -Force
    }
} else {
    $name = "butterpollo-setup-$Version.exe"
    $source = Join-Path $SigningDirectory $name
    Assert-Signed $source
    Copy-Item -LiteralPath $source -Destination (Join-Path $TargetDirectory $name) -Force
}
