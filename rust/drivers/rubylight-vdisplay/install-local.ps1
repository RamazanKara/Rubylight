# Installs Rubylight's display driver package on this PC. Elevated (setup runs it as SYSTEM).
#
# An unsigned package (build-package.ps1 without -TestSign, and every release until SignPath
# signs the catalog) is signed here with a key that exists only for the length of this script.
# A UMDF driver needs no Microsoft signature: Plug and Play installs a package whose catalog
# chains to a root in LocalMachine\Root and whose signer is in TrustedPublisher, and WUDFHost
# loads the DLL (no embedded signature needed without /INTEGRITYCHECK). So this script
#   1. creates a self-signed code-signing certificate with a non-exportable key,
#   2. signs the package's catalog with it,
#   3. trusts the certificate (Root and TrustedPublisher) and deletes the private key,
#   4. installs the package and creates the Root\RubylightVDisplay device if it is missing,
#   5. removes the certificate of the previous local install.
# Once the key is gone nobody, Rubylight included, can sign anything this PC trusts with it.
#
# A package whose catalog SignPath signed is installed as it is: only its publisher
# certificate goes into TrustedPublisher (so Windows does not ask), and a locally signed
# install before it is cleaned up as above.
#
# -Uninstall removes the device, the packages and the certificates this script added.
# -InstallerBestEffort reports a failure as a DRIVER_WARNING line and exits 0, as the Sunshine
# driver's install.ps1 does for setup.
param(
    # The folder with rubylight_vdisplay.inf/.dll/.cat: this script's own in a Rubylight
    # package, target\package in the source tree.
    [string] $Package = $(if (Test-Path -LiteralPath (Join-Path $PSScriptRoot 'rubylight_vdisplay.inf')) { $PSScriptRoot } else { Join-Path $PSScriptRoot 'target\package' }),
    # Creates the root device; a Rubylight package carries it next to this script.
    [string] $NefConc = $(if (Test-Path -LiteralPath (Join-Path $PSScriptRoot 'nefconc.exe')) { Join-Path $PSScriptRoot 'nefconc.exe' }),
    [switch] $Uninstall,
    [switch] $InstallerBestEffort
)
$ErrorActionPreference = 'Stop'
$hardwareId = 'Root\RubylightVDisplay'
$classGuid = '{4D36E968-E325-11CE-BFC1-08002BE10318}'
$state = 'HKLM:\SOFTWARE\Rubylight\VirtualDisplay'
$subject = 'CN=Rubylight Virtual Display (signed on this PC)'
$script:restart = $false

function Remove-Trust([string] $thumbprint) {
    if (-not $thumbprint) { return }
    foreach ($store in 'Root', 'TrustedPublisher') {
        Get-Item "Cert:\LocalMachine\$store\$thumbprint" -ErrorAction SilentlyContinue | Remove-Item
    }
}
function Add-Trust([Security.Cryptography.X509Certificates.X509Certificate2] $certificate, [string[]] $stores) {
    # The public part only: no key ever reaches these stores.
    $public = [Security.Cryptography.X509Certificates.X509Certificate2]::new($certificate.RawData)
    foreach ($name in $stores) {
        $store = [Security.Cryptography.X509Certificates.X509Store]::new($name, 'LocalMachine')
        $store.Open('ReadWrite'); $store.Add($public); $store.Close()
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
    if ($LASTEXITCODE -notin $ok) { throw "$(Split-Path -Leaf $exe) $($arguments -join ' ') failed with exit code $LASTEXITCODE" }
}
function Add-Package([string] $inf) {
    Invoke-Checked pnputil @('/add-driver', $inf, '/install') @(0, 259, 3010)
    if ($LASTEXITCODE -eq 3010) { $script:restart = $true }
}
# Signs the catalog with a new key that is deleted before this returns; returns the certificate.
function Protect-Catalog([string] $catalog) {
    $signer = New-SelfSignedCertificate -Type CodeSigningCert -Subject $subject -CertStoreLocation Cert:\LocalMachine\My `
        -KeyExportPolicy NonExportable -KeyAlgorithm RSA -KeyLength 2048 -HashAlgorithm SHA256 -NotAfter (Get-Date).AddYears(50)
    try {
        $signature = Set-AuthenticodeSignature -LiteralPath $catalog -Certificate $signer -HashAlgorithm SHA256
        if ($signature.Status -notin 'Valid', 'UnknownError') { throw "signing the catalog failed: $($signature.StatusMessage)" }
        Add-Trust $signer 'Root', 'TrustedPublisher'
    } finally {
        # The key goes whatever happened above.
        Remove-Item "Cert:\LocalMachine\My\$($signer.Thumbprint)" -DeleteKey
    }
    if ((Get-AuthenticodeSignature -LiteralPath $catalog).Status -ne 'Valid') {
        Remove-Trust $signer.Thumbprint
        throw 'the signed catalog does not verify'
    }
    $signer
}

function Install-Package {
    $recorded = Get-ItemProperty $state -ErrorAction SilentlyContinue
    $previous = $recorded.LocalSigner
    if ($Uninstall) {
        Get-Device | ForEach-Object { Invoke-Checked pnputil @('/remove-device', $_.InstanceId) }
        foreach ($inf in Get-Packages) { Invoke-Checked pnputil @('/delete-driver', $inf, '/uninstall', '/force') }
        Remove-Trust $previous
        if ($recorded.PublisherAdded) {
            Get-Item "Cert:\LocalMachine\TrustedPublisher\$($recorded.PublisherAdded)" -ErrorAction SilentlyContinue | Remove-Item
        }
        Remove-Item $state -ErrorAction SilentlyContinue
        return
    }

    $catalog = Join-Path $Package 'rubylight_vdisplay.cat'
    $status = (Get-AuthenticodeSignature -LiteralPath $catalog).Status
    if ($status -notin 'NotSigned', 'Valid') { throw "$catalog has a signature Windows rejects ($status)" }
    $work = Join-Path ([IO.Path]::GetTempPath()) "rubylight-vdisplay-$([guid]::NewGuid())"
    New-Item -ItemType Directory $work | Out-Null
    foreach ($extension in 'inf', 'dll', 'cat') { Copy-Item -LiteralPath (Join-Path $Package "rubylight_vdisplay.$extension") $work }
    $signer = $null
    $publisherAdded = $recorded.PublisherAdded
    try {
        if ($status -eq 'Valid') {
            # Signed by SignPath: trust that publisher for driver installs, nothing more.
            $publisher = (Get-AuthenticodeSignature -LiteralPath (Join-Path $work 'rubylight_vdisplay.cat')).SignerCertificate
            if (-not (Test-Path "Cert:\LocalMachine\TrustedPublisher\$($publisher.Thumbprint)")) {
                Add-Trust $publisher 'TrustedPublisher'
                $publisherAdded = $publisher.Thumbprint
            }
        } else {
            $signer = Protect-Catalog (Join-Path $work 'rubylight_vdisplay.cat')
        }
        $before = @(Get-Packages)
        Add-Package (Join-Path $work 'rubylight_vdisplay.inf')
        if (-not (Get-Device)) {
            if (-not $NefConc) { throw "no $hardwareId device: pass -NefConc to create it" }
            Invoke-Checked $NefConc @('--create-device-node', '--class-name', 'Display', '--class-guid', $classGuid, '--hardware-id', $hardwareId)
            Add-Package (Join-Path $work 'rubylight_vdisplay.inf')
        }
    } catch {
        if ($signer) { Remove-Trust $signer.Thumbprint }
        throw
    } finally {
        Remove-Item -Recurse -Force $work
    }
    New-Item $state -Force | Out-Null
    # Setup and the service tell an installed driver by this key.
    Set-ItemProperty $state Installed ((Get-Date).ToUniversalTime().ToString('o'))
    if ($signer) {
        Set-ItemProperty $state LocalSigner $signer.Thumbprint
    } else {
        Remove-ItemProperty $state LocalSigner -ErrorAction SilentlyContinue
    }
    if ($publisherAdded) { Set-ItemProperty $state PublisherAdded $publisherAdded }
    # Older packages were signed by the previous local signer: delete them before its trust goes.
    $current = (Get-Device | Get-PnpDeviceProperty -KeyName DEVPKEY_Device_DriverInfPath).Data
    foreach ($inf in $before | Where-Object { $_ -ne $current }) { pnputil /delete-driver $inf | Out-Null }
    if ($previous -and $previous -ne $signer.Thumbprint) { Remove-Trust $previous }
    $device = @(Get-Device)[0]
    $device | Format-List InstanceId, Status, Problem | Out-Host
    if (-not $script:restart -and $device.Status -ne 'OK') {
        throw "the Rubylight display device reports $($device.Status) (problem $($device.Problem))"
    }
}

try {
    Install-Package
    if ($script:restart) { Write-Output 'RESTART_REQUIRED' }
} catch {
    if (-not $InstallerBestEffort) { throw }
    Write-Warning '[RubylightVirtualDisplay] DRIVER_WARNING: Rubylight display driver setup did not complete.'
    Write-Warning "[RubylightVirtualDisplay] Installer best-effort driver action failed: $($_.Exception.Message)"
}
