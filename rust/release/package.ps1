param(
    [Parameter(Mandatory)][string]$Distribution,
    [Parameter(Mandatory)][string]$TargetDirectory,
    [Parameter(Mandatory)][string]$Stub,
    [Parameter(Mandatory)][string]$Version,
    [ValidateSet('debug', 'release')][string]$ProfileName = 'release'
)
$ErrorActionPreference = 'Stop'
$Distribution = (Resolve-Path -LiteralPath $Distribution).ProviderPath
$TargetDirectory = (Resolve-Path -LiteralPath $TargetDirectory).ProviderPath
# This is run again after signing the individual binaries. Never hash the old
# manifest into the new manifest, and only then build the archive and installer.
$manifestPath = Join-Path $Distribution 'manifest.json'
$manifest = @(Get-ChildItem -LiteralPath $Distribution -File -Recurse |
    Where-Object { $_.FullName -ne $manifestPath } | Sort-Object FullName | ForEach-Object {
        [pscustomobject]@{
            path = $_.FullName.Substring($Distribution.Length + 1)
            sha256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        }
    })
ConvertTo-Json -InputObject $manifest | Set-Content -LiteralPath $manifestPath -Encoding utf8
$zip = Join-Path $TargetDirectory "butterpollo-rust-$ProfileName.zip"
Compress-Archive -LiteralPath $Distribution -DestinationPath $zip -Force
$installer = Join-Path $TargetDirectory "butterpollo-setup-$Version.exe"
$stream = [IO.File]::Create($installer)
try {
    foreach ($path in @($Stub, $zip)) {
        $source = [IO.File]::OpenRead($path)
        try { $source.CopyTo($stream) } finally { $source.Dispose() }
    }
    $stream.Write([Text.Encoding]::ASCII.GetBytes('BPSETUP1'), 0, 8)
    $stream.Write([BitConverter]::GetBytes([uint64](Get-Item -LiteralPath $zip).Length), 0, 8)
} finally { $stream.Dispose() }
Write-Output "Package: $zip"
Write-Output "Installer: $installer"
