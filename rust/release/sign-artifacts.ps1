param(
    [Parameter(Mandatory)][ValidateSet('StageBinaries', 'ApplyBinaries', 'ApplyInstaller')][string]$Phase,
    [Parameter(Mandatory)][string]$TargetDirectory,
    [Parameter(Mandatory)][string]$SigningDirectory,
    [Parameter(Mandatory)][string]$Version
)
$ErrorActionPreference = 'Stop'
$distribution = Join-Path $TargetDirectory 'butterpollo-rust-release'
$binaries = @(
    'butterpollo.exe', 'butterpollo-service.exe', 'Start Rubylight.exe',
    'butterpollo-setup.exe', 'butterpollo-performance.exe',
    'butterpollo-protocol-performance.exe', 'butterpollo-video-packet-performance.exe',
    'butterpollo_truehdr.dll', 'vulkan-layer/butterpollo_vulkan_layer.dll'
)
function Assert-Signed([string]$Path) {
    $signature = Get-AuthenticodeSignature -LiteralPath $Path
    if ($signature.Status -ne 'Valid' -or !$signature.SignerCertificate -or !$signature.TimeStamperCertificate) {
        throw "Missing valid timestamped Authenticode signature: $Path ($($signature.Status))"
    }
}
if ($Phase -eq 'StageBinaries') {
    foreach ($name in $binaries) {
        $source = Join-Path $distribution $name
        if (!(Test-Path -LiteralPath $source -PathType Leaf)) { throw "Missing release binary: $name" }
        $target = Join-Path $SigningDirectory $name
        New-Item -ItemType Directory -Path (Split-Path -Parent $target) -Force | Out-Null
        Copy-Item -LiteralPath $source -Destination $target
    }
} elseif ($Phase -eq 'ApplyBinaries') {
    # Verify the complete set before replacing anything. Third-party binaries
    # retain their upstream signatures and are never submitted for re-signing.
    foreach ($name in $binaries) { Assert-Signed (Join-Path $SigningDirectory $name) }
    foreach ($name in $binaries) {
        Copy-Item -LiteralPath (Join-Path $SigningDirectory $name) -Destination (Join-Path $distribution $name) -Force
    }
} else {
    $name = "butterpollo-setup-$Version.exe"
    $source = Join-Path $SigningDirectory $name
    Assert-Signed $source
    Copy-Item -LiteralPath $source -Destination (Join-Path $TargetDirectory $name) -Force
}
