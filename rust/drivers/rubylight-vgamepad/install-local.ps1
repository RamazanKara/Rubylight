# Installs Rubylight's virtual gamepad driver package on this PC. Elevated (setup runs it as SYSTEM).
#
# Mirrors rust/drivers/rubylight-vdisplay/install-local.ps1:
#   - An unsigned catalog (development builds and unsigned releases) is signed here with an
#     ephemeral non-exportable certificate trusted in LocalMachine\Root and TrustedPublisher
#     whose private key is immediately deleted.
#   - A signed release catalog is installed as it is, trusting only its publisher certificate
#     in TrustedPublisher.
#   - Under Smart App Control, S mode, or enforced App Control (UMCI), an unsigned package
#     is refused cleanly and recorded under HKLM:\SOFTWARE\Rubylight\VirtualGamepad\Problem.
#   - -Uninstall removes the Root\RubylightVirtualGamepad device, INF packages and certificates.
param(
    [string] $Package = $(if (Test-Path -LiteralPath (Join-Path $PSScriptRoot 'rubylight_vgamepad.inf')) { $PSScriptRoot } else { Join-Path $PSScriptRoot 'target\package' }),
    [string] $NefConc = $(if (Test-Path -LiteralPath (Join-Path $PSScriptRoot 'nefconc.exe')) { Join-Path $PSScriptRoot 'nefconc.exe' }),
    [switch] $Uninstall,
    [switch] $InstallerBestEffort
)
$ErrorActionPreference = 'Stop'
$hardwareId = 'Root\RubylightVirtualGamepad'
$classGuid = '{4d36e97d-e325-11ce-bfc1-08002be10318}'
$state = 'HKLM:\SOFTWARE\Rubylight\VirtualGamepad'
$subject = 'CN=Rubylight Virtual Gamepad (signed on this PC)'
$script:restart = $false

function Remove-Trust([string] $thumbprint) {
    if (-not $thumbprint) { return }
    foreach ($store in 'Root', 'TrustedPublisher') {
        Get-Item "Cert:\LocalMachine\$store\$thumbprint" -ErrorAction SilentlyContinue | Remove-Item
    }
}
function Add-Trust([Security.Cryptography.X509Certificates.X509Certificate2] $certificate, [string[]] $stores) {
    $public = [Security.Cryptography.X509Certificates.X509Certificate2]::new($certificate.RawData)
    foreach ($name in $stores) {
        $store = [Security.Cryptography.X509Certificates.X509Store]::new($name, 'LocalMachine')
        $store.Open('ReadWrite'); $store.Add($public); $store.Close()
    }
}
function Get-Packages {
    Get-ChildItem "$env:SystemRoot\INF\oem*.inf" | Where-Object {
        Select-String -LiteralPath $_.FullName -Pattern '^\s*CatalogFile\s*=\s*rubylight_vgamepad\.cat' -Quiet
    } | ForEach-Object Name
}
function Get-Device {
    Get-PnpDevice -ErrorAction SilentlyContinue | Where-Object { $_.HardwareID -contains $hardwareId }
}
function Invoke-Checked([string] $exe, [string[]] $arguments, [int[]] $ok = @(0)) {
    & $exe @arguments | Out-Host
    if ($LASTEXITCODE -notin $ok) { throw "$(Split-Path -Leaf $exe) $($arguments -join ' ') failed with exit code $LASTEXITCODE" }
}
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
function Protect-Catalog([string] $catalog) {
    $signer = New-SelfSignedCertificate -Type CodeSigningCert -Subject $subject -CertStoreLocation Cert:\LocalMachine\My `
        -KeyExportPolicy NonExportable -KeyAlgorithm RSA -KeyLength 2048 -HashAlgorithm SHA256 -NotAfter (Get-Date).AddYears(50)
    try {
        $signature = Set-AuthenticodeSignature -LiteralPath $catalog -Certificate $signer -HashAlgorithm SHA256
        if ($signature.Status -notin 'Valid', 'UnknownError') { throw "signing the catalog failed: $($signature.StatusMessage)" }
        Add-Trust $signer 'Root', 'TrustedPublisher'
    } finally {
        Remove-Item "Cert:\LocalMachine\My\$($signer.Thumbprint)" -DeleteKey
    }
    if ((Get-AuthenticodeSignature -LiteralPath $catalog).Status -ne 'Valid') {
        Remove-Trust $signer.Thumbprint
        throw 'the signed catalog does not verify'
    }
    $signer
}
function Get-Refusal {
    $policy = Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\CI\Policy' -ErrorAction SilentlyContinue
    if ($policy.VerifiedAndReputablePolicyState -eq 1) {
        return 'Smart App Control is on, and it refuses a virtual gamepad driver signed on this PC, so controller input is ignored. A signed Rubylight release will install the driver. Turning Smart App Control off (Windows Security, App and browser control) and running the Rubylight installer again installs it now, but Windows cannot turn Smart App Control back on without a reset.'
    }
    if ($policy.SkuPolicyRequired -eq 1) {
        return 'Windows runs in S mode, which refuses a virtual gamepad driver signed on this PC, so controller input is ignored. Switching out of S mode (free, in the Microsoft Store) and running the Rubylight installer again installs it.'
    }
    $guard = Get-CimInstance -Namespace root\Microsoft\Windows\DeviceGuard -ClassName Win32_DeviceGuard -ErrorAction SilentlyContinue
    if ($guard.UsermodeCodeIntegrityPolicyEnforcementStatus -eq 2) {
        return 'An App Control policy on this PC refuses a virtual gamepad driver signed on this PC, so controller input is ignored. Ask whoever manages this PC to allow Rubylight''s virtual gamepad driver, then run the Rubylight installer again.'
    }
}
function Remove-Install($recorded) {
    Get-Device | ForEach-Object { Invoke-Restartable @('/remove-device', $_.InstanceId) }
    foreach ($inf in Get-Packages) { Invoke-Restartable @('/delete-driver', $inf, '/uninstall', '/force') }
    Remove-Trust $recorded.LocalSigner
    if ($recorded.PublisherAdded) {
        Get-Item "Cert:\LocalMachine\TrustedPublisher\$($recorded.PublisherAdded)" -ErrorAction SilentlyContinue | Remove-Item
    }
    Remove-Item $state -ErrorAction SilentlyContinue
}
function Set-Problem([string] $text) {
    New-Item $state -Force | Out-Null
    Set-ItemProperty $state Problem $text
}
$script:unsupported = $null

function Install-Package {
    $recorded = Get-ItemProperty $state -ErrorAction SilentlyContinue
    $previous = $recorded.LocalSigner
    if ($Uninstall) {
        Remove-Install $recorded
        return
    }

    $catalog = Join-Path $Package 'rubylight_vgamepad.cat'
    $status = (Get-AuthenticodeSignature -LiteralPath $catalog).Status
    if ($status -notin 'NotSigned', 'Valid') { throw "$catalog has a signature Windows rejects ($status)" }
    $refusal = if ($status -eq 'NotSigned') { Get-Refusal }
    if ($refusal) {
        if ($recorded -or (Get-Device) -or (Get-Packages)) { Remove-Install $recorded }
        Set-Problem $refusal
        $script:unsupported = $refusal
        return
    }
    $work = Join-Path ([IO.Path]::GetTempPath()) "rubylight-vgamepad-$([guid]::NewGuid())"
    New-Item -ItemType Directory $work | Out-Null
    foreach ($extension in 'inf', 'dll', 'cat') { Copy-Item -LiteralPath (Join-Path $Package "rubylight_vgamepad.$extension") $work }
    $signer = $null
    $publisherAdded = $recorded.PublisherAdded
    $before = @(Get-Packages)
    $hadDevice = [bool](Get-Device)
    try {
        if ($status -eq 'Valid') {
            $publisher = (Get-AuthenticodeSignature -LiteralPath (Join-Path $work 'rubylight_vgamepad.cat')).SignerCertificate
            if (-not (Test-Path "Cert:\LocalMachine\TrustedPublisher\$($publisher.Thumbprint)")) {
                Add-Trust $publisher 'TrustedPublisher'
                $publisherAdded = $publisher.Thumbprint
            }
        } else {
            $signer = Protect-Catalog (Join-Path $work 'rubylight_vgamepad.cat')
        }
        $inf = Join-Path $work 'rubylight_vgamepad.inf'
        Add-Package $inf
        if (-not (Get-Device)) {
            if (-not $NefConc) { throw "no $hardwareId device: pass -NefConc to create it" }
            Invoke-Checked $NefConc @('--create-device-node', '--class-name', 'System', '--class-guid', $classGuid, '--hardware-id', $hardwareId)
            Add-Package $inf
        }
        $hash = (Get-FileHash -LiteralPath $inf).Hash
        $ours = Get-Packages | Where-Object { (Get-FileHash -LiteralPath "$env:SystemRoot\INF\$_").Hash -eq $hash } | Select-Object -First 1
        if (-not $ours) { throw 'the package is missing from the driver store after pnputil added it' }
        if ((Get-CurrentPackage) -ne $ours) {
            foreach ($other in @(Get-Packages | Where-Object { $_ -ne $ours })) {
                Invoke-Restartable @('/delete-driver', $other, '/uninstall', '/force')
            }
            Add-Package $inf
            if (-not $script:restart -and (Get-CurrentPackage) -ne $ours) { throw "the device uses $(Get-CurrentPackage), not $ours" }
        }
        $device = @(Get-Device)[0]
        $device | Format-List InstanceId, Status, Problem | Out-Host
        if (-not $script:restart -and $device.Status -ne 'OK') {
            throw "Windows did not start the virtual gamepad driver ($($device.Status), problem $($device.Problem))"
        }
    } catch {
        $failure = $_
        try {
            if (-not $hadDevice) { Get-Device | ForEach-Object { pnputil /remove-device $_.InstanceId | Out-Null } }
            foreach ($added in @(Get-Packages | Where-Object { $_ -notin $before })) {
                pnputil /delete-driver $added /uninstall /force | Out-Null
            }
        } catch { Write-Host "rolling back the virtual gamepad driver install: $_" }
        if ($signer) { Remove-Trust $signer.Thumbprint }
        if (-not $hadDevice) {
            $reason = $failure.Exception
            while ($reason.InnerException) { $reason = $reason.InnerException }
            Set-Problem "Rubylight's virtual gamepad driver could not be installed ($($reason.Message -replace '\s+', ' ')), so controller input is ignored. Run the Rubylight installer again; its log has the details."
        }
        throw $failure
    } finally {
        Remove-Item -Recurse -Force $work
    }
    New-Item $state -Force | Out-Null
    Set-ItemProperty $state Installed ((Get-Date).ToUniversalTime().ToString('o'))
    if ($signer) {
        Set-ItemProperty $state LocalSigner $signer.Thumbprint
    } else {
        Remove-ItemProperty $state LocalSigner -ErrorAction SilentlyContinue
    }
    if ($publisherAdded) { Set-ItemProperty $state PublisherAdded $publisherAdded }
    Remove-ItemProperty $state Problem -ErrorAction SilentlyContinue
    foreach ($other in @(Get-Packages | Where-Object { $_ -ne $ours })) { pnputil /delete-driver $other | Out-Null }
    if ($previous -and $previous -ne $signer.Thumbprint) { Remove-Trust $previous }
}

try {
    Install-Package
    if ($script:unsupported) {
        if (-not $InstallerBestEffort) { throw $script:unsupported }
        Write-Warning "[RubylightVirtualGamepad] DRIVER_UNSUPPORTED: $script:unsupported"
    }
} catch {
    if (-not $InstallerBestEffort) { throw }
    Write-Warning '[RubylightVirtualGamepad] DRIVER_WARNING: Rubylight virtual gamepad driver setup did not complete.'
    $reason = $_.Exception
    while ($reason.InnerException) { $reason = $reason.InnerException }
    Write-Warning "[RubylightVirtualGamepad] Installer best-effort driver action failed: $($reason.Message -replace '\s+', ' ')"
}
if ($script:restart) { Write-Output 'RESTART_REQUIRED' }
