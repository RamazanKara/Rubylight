[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Distribution,
    [Parameter(Mandatory)][string]$Zip,
    [Parameter(Mandatory)][string]$Installer
)
$ErrorActionPreference = 'Stop'
$Distribution = (Resolve-Path -LiteralPath $Distribution).ProviderPath

# Keep the original, unsigned setup stub; the rebuilt installer is signed last.
$original = [IO.File]::ReadAllBytes($Installer)
if ($original.Length -lt 16 -or [Text.Encoding]::ASCII.GetString($original, $original.Length - 16, 8) -ne 'BPSETUP1') {
    throw 'Expected an unsigned installer with a package footer'
}
$length = [BitConverter]::ToUInt64($original, $original.Length - 8)
if ($length -eq 0 -or $length -ge $original.Length - 16) { throw 'Invalid setup payload length' }
$stubLength = $original.Length - 16 - $length

$manifest = Get-ChildItem -LiteralPath $Distribution -File -Recurse | Where-Object { $_.FullName -ne (Join-Path $Distribution 'manifest.json') } | ForEach-Object {
    [pscustomobject]@{ path = $_.FullName.Substring($Distribution.Length + 1); sha256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant() }
}
$manifest | ConvertTo-Json | Set-Content -LiteralPath "$Distribution\manifest.json" -Encoding utf8
Compress-Archive -LiteralPath $Distribution -DestinationPath $Zip -Force
$archive = [IO.File]::ReadAllBytes($Zip)
$stream = [IO.File]::Create($Installer)
try {
    $stream.Write($original, 0, $stubLength)
    $stream.Write($archive, 0, $archive.Length)
    $stream.Write([Text.Encoding]::ASCII.GetBytes('BPSETUP1'), 0, 8)
    $stream.Write([BitConverter]::GetBytes([uint64]$archive.Length), 0, 8)
} finally { $stream.Dispose() }
