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
# On a PC whose policy refuses a driver signed here (Smart App Control, S mode, App Control),
# it installs nothing, removes an earlier install and records why (Problem) for setup and the
# console. A failed fresh install is rolled back and recorded the same way.
#
# -CheckOnly changes nothing: it prints DRIVER_UNSUPPORTED with the reason when this PC would
# refuse the package, else DRIVER_SUPPORTED. Setup asks before it touches anything.
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
    [switch] $CheckOnly,
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
# pnputil, where 3010 means done once Windows restarts.
function Invoke-Restartable([string[]] $arguments, [int[]] $ok = @(0, 3010)) {
    Invoke-Checked pnputil $arguments $ok
    if ($LASTEXITCODE -eq 3010) { $script:restart = $true }
}
function Get-CurrentPackage {
    (Get-Device | Get-PnpDeviceProperty -KeyName DEVPKEY_Device_DriverInfPath).Data
}
function Add-Package([string] $inf) {
    Invoke-Restartable @('/add-driver', $inf, '/install') @(0, 259, 3010)
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

# Why Windows will not load a driver signed on this PC, and what the user can do, or nothing:
# Smart App Control (on, not evaluating), S mode, or an App Control for Business (WDAC) policy
# enforcing user-mode code integrity. Each admits only publishers it already knows, never a
# certificate made here. The text is what setup's note and the console show.
#   SkuPolicyRequired (HKLM\SYSTEM\CurrentControlSet\Control\CI\Policy): 1 = S mode, 0 or
#     absent = not; Microsoft-Windows-CodeIntegrity-SkuPolicyRequired, learn.microsoft.com/
#     windows-hardware/customize/desktop/unattend/microsoft-windows-codeintegrity-skupolicyrequired
#   Win32_DeviceGuard.UsermodeCodeIntegrityPolicyEnforcementStatus: 0 off, 1 audit, 2 enforced,
#     as its CodeIntegrityPolicyEnforcementStatus sibling on learn.microsoft.com/windows/
#     security/hardware-security/enable-virtualization-based-protection-of-code-integrity
#     (the user-mode field itself is named in DISA's Windows STIG checks, not on that page).
#     Memory Integrity alone enforces only the kernel-mode one, which this does not read.
#   VerifiedAndReputablePolicyState (same key): 0 off, 1 on (enforce), 2 evaluation. Microsoft
#     does not document this value; the meaning is the one Windows Security's own toggle
#     writes and that published guides agree on. Evaluation never blocks, so only 1 counts.
function Get-Refusal {
    $policy = Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\CI\Policy' -ErrorAction SilentlyContinue
    if ($policy.VerifiedAndReputablePolicyState -eq 1) {
        return 'Smart App Control is on, and it refuses a display driver signed on this PC, and Rubylight streams only through it. A signed Rubylight release will install the driver. Turning Smart App Control off (Windows Security, App and browser control) and running the Rubylight installer again installs it now, but Windows cannot turn Smart App Control back on without a reset.'
    }
    if ($policy.SkuPolicyRequired -eq 1) {
        return 'Windows runs in S mode, which refuses a display driver signed on this PC, and Rubylight streams only through it. Switching out of S mode (free, in the Microsoft Store) and running the Rubylight installer again installs it.'
    }
    $guard = Get-CimInstance -Namespace root\Microsoft\Windows\DeviceGuard -ClassName Win32_DeviceGuard -ErrorAction SilentlyContinue
    if ($guard.UsermodeCodeIntegrityPolicyEnforcementStatus -eq 2) {
        return 'An App Control policy on this PC refuses a display driver signed on this PC, and Rubylight streams only through it. Ask whoever manages this PC to allow Rubylight''s display driver, then run the Rubylight installer again.'
    }
}
# Remove the device, every package and the certificates a recorded install added.
function Remove-Install($recorded) {
    # 3010: removed, but a device in use (a display the host still drives) goes at the
    # next restart.
    Get-Device | ForEach-Object { Invoke-Restartable @('/remove-device', $_.InstanceId) }
    foreach ($inf in Get-Packages) { Invoke-Restartable @('/delete-driver', $inf, '/uninstall', '/force') }
    Remove-Trust $recorded.LocalSigner
    if ($recorded.PublisherAdded) {
        Get-Item "Cert:\LocalMachine\TrustedPublisher\$($recorded.PublisherAdded)" -ErrorAction SilentlyContinue | Remove-Item
    }
    Remove-Item $state -ErrorAction SilentlyContinue
}
# Why there is no driver, for the console (the host reads it when the driver does not open).
function Set-Problem([string] $text) {
    New-Item $state -Force | Out-Null
    Set-ItemProperty $state Problem $text
}
$script:unsupported = $null
$script:unsupported = $null

function Install-Package {
    $recorded = Get-ItemProperty $state -ErrorAction SilentlyContinue
    $previous = $recorded.LocalSigner
    if ($Uninstall) {
        Remove-Install $recorded
        return
    }

    $catalog = Join-Path $Package 'rubylight_vdisplay.cat'
    $status = (Get-AuthenticodeSignature -LiteralPath $catalog).Status
    if ($status -notin 'NotSigned', 'Valid') { throw "$catalog has a signature Windows rejects ($status)" }
    $refusal = if ($status -eq 'NotSigned') { Get-Refusal }
    if ($refusal) {
        # Before any certificate is made. An earlier install (Smart App Control can be on
        # since) leaves no device that cannot load behind.
        if ($recorded -or (Get-Device) -or (Get-Packages)) { Remove-Install $recorded }
        Set-Problem $refusal
        $script:unsupported = $refusal
        return
    }
    $work = Join-Path ([IO.Path]::GetTempPath()) "rubylight-vdisplay-$([guid]::NewGuid())"
    New-Item -ItemType Directory $work | Out-Null
    foreach ($extension in 'inf', 'dll', 'cat') { Copy-Item -LiteralPath (Join-Path $Package "rubylight_vdisplay.$extension") $work }
    $signer = $null
    $publisherAdded = $recorded.PublisherAdded
    # What a failed install takes back: the packages and the device it added.
    $before = @(Get-Packages)
    $hadDevice = [bool](Get-Device)
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
        $inf = Join-Path $work 'rubylight_vdisplay.inf'
        Add-Package $inf
        if (-not (Get-Device)) {
            if (-not $NefConc) { throw "no $hardwareId device: pass -NefConc to create it" }
            Invoke-Checked $NefConc @('--create-device-node', '--class-name', 'Display', '--class-guid', $classGuid, '--hardware-id', $hardwareId)
            Add-Package $inf
        }
        # The driver store keeps the INF as it is: this package is the one with its hash.
        $hash = (Get-FileHash -LiteralPath $inf).Hash
        $ours = Get-Packages | Where-Object { (Get-FileHash -LiteralPath "$env:SystemRoot\INF\$_").Hash -eq $hash } | Select-Object -First 1
        if (-not $ours) { throw 'the package is missing from the driver store after pnputil added it' }
        if ((Get-CurrentPackage) -ne $ours) {
            # A package that ranks higher (a newer build, or the newer Rubylight's after a
            # downgrade) keeps the device. Remove the others so it takes this one: the device
            # must not stay on a package whose signer loses its trust below.
            foreach ($other in @(Get-Packages | Where-Object { $_ -ne $ours })) {
                Invoke-Restartable @('/delete-driver', $other, '/uninstall', '/force')
            }
            Add-Package $inf
            if (-not $script:restart -and (Get-CurrentPackage) -ne $ours) { throw "the device uses $(Get-CurrentPackage), not $ours" }
        }
        # A device that cannot start (a policy this script did not recognise refused the
        # driver) counts as a failed install: nothing half-installed stays.
        $device = @(Get-Device)[0]
        $device | Format-List InstanceId, Status, Problem | Out-Host
        if (-not $script:restart -and $device.Status -ne 'OK') {
            throw "Windows did not start the display driver ($($device.Status), problem $($device.Problem))"
        }
    } catch {
        $failure = $_
        try {
            if (-not $hadDevice) { Get-Device | ForEach-Object { pnputil /remove-device $_.InstanceId | Out-Null } }
            foreach ($added in @(Get-Packages | Where-Object { $_ -notin $before })) {
                pnputil /delete-driver $added /uninstall /force | Out-Null
            }
        } catch { Write-Host "rolling back the display driver install: $_" }
        if ($signer) { Remove-Trust $signer.Thumbprint }
        if (-not $hadDevice) {
            $reason = $failure.Exception
            while ($reason.InnerException) { $reason = $reason.InnerException }
            Set-Problem "Rubylight's display driver could not be installed ($($reason.Message -replace '\s+', ' ')), and Rubylight streams only through it. Run the Rubylight installer again; its log has the details."
        }
        throw $failure
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
    foreach ($other in @(Get-Packages | Where-Object { $_ -ne $ours })) { pnputil /delete-driver $other | Out-Null }
    if ($previous -and $previous -ne $signer.Thumbprint) { Remove-Trust $previous }
}

if ($CheckOnly) {
    $catalog = Join-Path $Package 'rubylight_vdisplay.cat'
    $status = (Get-AuthenticodeSignature -LiteralPath $catalog).Status
    $refusal = if ($status -notin 'NotSigned', 'Valid') {
        "the display driver's catalog has a signature Windows rejects ($status)"
    } elseif ($status -eq 'NotSigned') { Get-Refusal }
    if ($refusal) { Write-Output "DRIVER_UNSUPPORTED: $refusal" } else { Write-Output 'DRIVER_SUPPORTED' }
    exit 0
}
try {
    Install-Package
    if ($script:unsupported) {
        if (-not $InstallerBestEffort) { throw $script:unsupported }
        Write-Warning "[RubylightVirtualDisplay] DRIVER_UNSUPPORTED: $script:unsupported"
    }
} catch {
    if (-not $InstallerBestEffort) { throw }
    Write-Warning '[RubylightVirtualDisplay] DRIVER_WARNING: Rubylight display driver setup did not complete.'
    # One line: setup quotes the reason from it. The innermost message names the failed step.
    $reason = $_.Exception
    while ($reason.InnerException) { $reason = $reason.InnerException }
    Write-Warning "[RubylightVirtualDisplay] Installer best-effort driver action failed: $($reason.Message -replace '\s+', ' ')"
}
# Also after a failure: what pnputil did may still need the restart.
if ($script:restart) { Write-Output 'RESTART_REQUIRED' }
