# Installs an unsigned driver package (build-package.ps1 without -TestSign) on this PC,
# signed here with a key that exists only for the length of this script.
#
# A UMDF driver needs no Microsoft signature: Plug and Play installs a package whose catalog
# chains to a root in LocalMachine\Root and whose signer is in TrustedPublisher, and WUDFHost
# loads the DLL (no embedded signature needed without /INTEGRITYCHECK). So this script
#   1. creates a self-signed code-signing certificate with a non-exportable key,
#   2. signs the package's catalog with it,
#   3. trusts the certificate (Root and TrustedPublisher) and deletes the private key,
#   4. installs the package and creates the Root\RubylightVDisplay device if it is missing,
#   5. removes the certificate of the previous local install.
# Once the key is gone nobody, Rubylight included, can sign anything this PC trusts with it.
# Elevated. -Uninstall removes the device, the packages and the certificate.
param(
    [string] $Package = (Join-Path $PSScriptRoot 'target\package'),
    [string] $NefConc,
    [switch] $Uninstall
)
$ErrorActionPreference = 'Stop'
$hardwareId = 'Root\RubylightVDisplay'
$classGuid = '{4D36E968-E325-11CE-BFC1-08002BE10318}'
$state = 'HKLM:\SOFTWARE\Rubylight\VirtualDisplay'
$subject = 'CN=Rubylight Virtual Display (signed on this PC)'

function Remove-Trust([string] $thumbprint) {
    if (-not $thumbprint) { return }
    foreach ($store in 'Root', 'TrustedPublisher') {
        Get-Item "Cert:\LocalMachine\$store\$thumbprint" -ErrorAction SilentlyContinue | Remove-Item
    }
}
function Get-Packages {
    # pnputil's output is localized; the INF cache names the original file.
    Get-ChildItem "$env:SystemRoot\INF\oem*.inf" | Where-Object {
        Select-String -LiteralPath $_.FullName -Pattern '^\s*CatalogFile\s*=\s*rubylight_vdisplay\.cat' -Quiet
    } | ForEach-Object Name
}
function Get-Device {
    Get-PnpDevice -Class Display -ErrorAction SilentlyContinue | Where-Object { $_.HardwareID -contains $hardwareId }
}
function Invoke-Checked([string] $exe, [string[]] $arguments, [int[]] $ok = @(0)) {
    & $exe @arguments | Out-Host
    if ($LASTEXITCODE -notin $ok) { throw "$(Split-Path -Leaf $exe) $($arguments -join ' ') failed with $LASTEXITCODE" }
}

$previous = (Get-ItemProperty $state -ErrorAction SilentlyContinue).LocalSigner
if ($Uninstall) {
    Get-Device | ForEach-Object { Invoke-Checked pnputil @('/remove-device', $_.InstanceId) }
    foreach ($inf in Get-Packages) { Invoke-Checked pnputil @('/delete-driver', $inf, '/uninstall', '/force') }
    Remove-Trust $previous
    Remove-Item $state -ErrorAction SilentlyContinue
    return
}

$catalog = Join-Path $Package 'rubylight_vdisplay.cat'
if ((Get-AuthenticodeSignature -LiteralPath $catalog).Status -ne 'NotSigned') { throw "$catalog is already signed" }

$work = Join-Path ([IO.Path]::GetTempPath()) "rubylight-vdisplay-$([guid]::NewGuid())"
Copy-Item -Recurse $Package $work
$signer = New-SelfSignedCertificate -Type CodeSigningCert -Subject $subject -CertStoreLocation Cert:\LocalMachine\My `
    -KeyExportPolicy NonExportable -KeyAlgorithm RSA -KeyLength 2048 -HashAlgorithm SHA256 -NotAfter (Get-Date).AddYears(50)
try {
    $signature = Set-AuthenticodeSignature -LiteralPath (Join-Path $work 'rubylight_vdisplay.cat') -Certificate $signer -HashAlgorithm SHA256
    if ($signature.Status -notin 'Valid', 'UnknownError') { throw "signing the catalog failed: $($signature.StatusMessage)" }
    $public = [Security.Cryptography.X509Certificates.X509Certificate2]::new($signer.RawData)
    foreach ($name in 'Root', 'TrustedPublisher') {
        $store = [Security.Cryptography.X509Certificates.X509Store]::new($name, 'LocalMachine')
        $store.Open('ReadWrite'); $store.Add($public); $store.Close()
    }
} finally {
    # The key goes whatever happened above.
    Remove-Item "Cert:\LocalMachine\My\$($signer.Thumbprint)" -DeleteKey
}
if ((Get-AuthenticodeSignature -LiteralPath (Join-Path $work 'rubylight_vdisplay.cat')).Status -ne 'Valid') {
    Remove-Trust $signer.Thumbprint
    throw 'the signed catalog does not verify'
}

try {
    $before = @(Get-Packages)
    Invoke-Checked pnputil @('/add-driver', (Join-Path $work 'rubylight_vdisplay.inf'), '/install') @(0, 259, 3010)
    if (-not (Get-Device)) {
        if (-not $NefConc) { throw "no $hardwareId device: pass -NefConc to create it" }
        Invoke-Checked $NefConc @('--create-device-node', '--class-name', 'Display', '--class-guid', $classGuid, '--hardware-id', $hardwareId)
        Invoke-Checked pnputil @('/add-driver', (Join-Path $work 'rubylight_vdisplay.inf'), '/install') @(0, 259, 3010)
    }
} catch {
    Remove-Trust $signer.Thumbprint
    throw
} finally {
    Remove-Item -Recurse -Force $work
}
New-Item $state -Force | Out-Null
Set-ItemProperty $state LocalSigner $signer.Thumbprint
# Older packages were signed by the previous local signer: delete them before its trust goes.
$current = (Get-Device | Get-PnpDeviceProperty -KeyName DEVPKEY_Device_DriverInfPath).Data
foreach ($inf in $before | Where-Object { $_ -ne $current }) { pnputil /delete-driver $inf | Out-Null }
if ($previous -and $previous -ne $signer.Thumbprint) { Remove-Trust $previous }
Get-Device | Format-List InstanceId, Status, Problem
