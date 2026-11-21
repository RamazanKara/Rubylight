<#
.SYNOPSIS
Build the Rubylight virtual gamepad driver and lay out its package (.inf, .dll, .cat).

.DESCRIPTION
Builds the driver with the MSVC toolchain and the Windows Driver Kit, stamps the INF, checks
it with InfVerif and generates the catalog with Inf2Cat, into target\package.

-TestSign signs the DLL and the catalog with a throwaway self-signed code-signing certificate
kept in target\signing (created on first use). The certificate is generated in memory and
written to files only: it is never imported into any certificate store, and nothing here
installs the driver or changes the machine.

.EXAMPLE
.\build-package.ps1 -TestSign
#>
param(
    [ValidateSet('release', 'debug')]
    [string]$BuildProfile = 'release',
    [switch]$TestSign,
    # With -TestSign: set the current test certificate's files aside and create a new one.
    [switch]$NewTestCertificate,
    # DriverVer version (w.x.y.z); defaults to the driver crate's version plus a build number.
    [string]$Version,
    [int]$Jobs = 4
)
$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
. "$root\driver-env.ps1"
Set-Location $root

function Invoke-Tool([string]$Exe, [string[]]$Arguments) {
    & $Exe @Arguments
    if ($LASTEXITCODE) { throw "$(Split-Path -Leaf $Exe) failed with exit code $LASTEXITCODE" }
}

$kit = (Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows Kits\Installed Roots').KitsRoot10
$sdk = Get-ChildItem "$kit\bin" -Directory | Where-Object {
    (Test-Path "$($_.FullName)\x64\signtool.exe") -and (Test-Path "$($_.FullName)\x86\Inf2Cat.exe")
} | Sort-Object { [version]$_.Name } | Select-Object -Last 1
if (-not $sdk) { throw 'The Windows Driver Kit (stampinf, Inf2Cat, signtool) is required' }
$bin = $sdk.FullName
$infverif = "$kit\Tools\$($sdk.Name)\x64\infverif.exe"

$cargoArgs = @('build', '--locked', '-p', 'rubylight-vgamepad', '-j', "$Jobs")
if ($BuildProfile -eq 'release') { $cargoArgs += '--release' }
Invoke-Tool cargo $cargoArgs

$built = "$root\target\x86_64-pc-windows-msvc\$BuildProfile"
$package = "$root\target\package"
if (Test-Path $package) { Remove-Item -Recurse -Force $package }
New-Item -ItemType Directory $package | Out-Null
Copy-Item "$built\rubylight_vgamepad.dll" $package
# IMAGE_DLLCHARACTERISTICS_FORCE_INTEGRITY (/INTEGRITYCHECK) makes WUDFHost refuse any DLL whose
# root the kernel does not know.
$image = [IO.File]::ReadAllBytes("$package\rubylight_vgamepad.dll")
$characteristics = [BitConverter]::ToUInt16($image, [BitConverter]::ToInt32($image, 0x3C) + 24 + 70)
if ($characteristics -band 0x80) { throw 'rubylight_vgamepad.dll is linked with /INTEGRITYCHECK' }
Copy-Item "$root\rubylight_vgamepad.inx" "$package\rubylight_vgamepad.inf"

if (-not $Version) {
    $crate = (Select-String -LiteralPath "$root\Cargo.toml" -Pattern '^version = "(\d+)\.(\d+)\.(\d+)"').Matches[0].Groups
    $build = [int]((Get-Date).ToUniversalTime() - [datetime]'2026-01-01').TotalHours
    $Version = "$($crate[1].Value).$($crate[2].Value).$($crate[3].Value).$build"
}
# UMDF 2.15 matches the crate's target-umdf-version-minor and Windows 10 19041+ support.
Invoke-Tool "$bin\x64\stampinf.exe" @('-f', "$package\rubylight_vgamepad.inf", '-a', 'amd64', '-d', '*', '-v', $Version, '-u', '2.15.0')
if (-not ((Get-Content -LiteralPath "$package\rubylight_vgamepad.inf") -match '^\s*UmdfLibraryVersion\s*=\s*2\.15\.0\s*$')) {
    throw 'rubylight_vgamepad.inf was not stamped with UmdfLibraryVersion = 2.15.0'
}
if (Test-Path $infverif) {
    Invoke-Tool $infverif @('/v', "$package\rubylight_vgamepad.inf")
}

$signing = "$root\target\signing"
$pfx = "$signing\rubylight-vgamepad-test.pfx"
$cer = "$signing\rubylight-vgamepad-test.cer"
$secret = "$signing\rubylight-vgamepad-test.password"
if ($TestSign) {
    if ($NewTestCertificate -and (Test-Path $pfx)) {
        $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
        foreach ($old in $pfx, $cer, $secret) {
            if (Test-Path $old) { Rename-Item -LiteralPath $old -NewName "$(Split-Path -Leaf $old).$stamp" }
        }
    }
    if (-not (Test-Path $pfx)) {
        New-Item -ItemType Directory -Force $signing | Out-Null
        $rsa = [System.Security.Cryptography.RSA]::Create(2048)
        $request = [System.Security.Cryptography.X509Certificates.CertificateRequest]::new(
            'CN=Rubylight Virtual Gamepad Test Signing (not trusted)', $rsa,
            [System.Security.Cryptography.HashAlgorithmName]::SHA256,
            [System.Security.Cryptography.RSASignaturePadding]::Pkcs1)
        $request.CertificateExtensions.Add(
            [System.Security.Cryptography.X509Certificates.X509KeyUsageExtension]::new(
                [System.Security.Cryptography.X509Certificates.X509KeyUsageFlags]::DigitalSignature, $true))
        $codeSigning = [System.Security.Cryptography.OidCollection]::new()
        $codeSigning.Add([System.Security.Cryptography.Oid]::new('1.3.6.1.5.5.7.3.3')) | Out-Null
        $request.CertificateExtensions.Add(
            [System.Security.Cryptography.X509Certificates.X509EnhancedKeyUsageExtension]::new($codeSigning, $false))
        $request.CertificateExtensions.Add(
            [System.Security.Cryptography.X509Certificates.X509SubjectKeyIdentifierExtension]::new($request.PublicKey, $false))
        $now = [DateTimeOffset]::UtcNow
        $cert = $request.CreateSelfSigned($now.AddDays(-1), $now.AddDays(90))
        $password = [Convert]::ToBase64String([System.Security.Cryptography.RandomNumberGenerator]::GetBytes(24))
        [IO.File]::WriteAllBytes($pfx, $cert.Export([System.Security.Cryptography.X509Certificates.X509ContentType]::Pfx, $password))
        [IO.File]::WriteAllBytes($cer, $cert.Export([System.Security.Cryptography.X509Certificates.X509ContentType]::Cert))
        Set-Content -LiteralPath $secret -Value $password -NoNewline
        Write-Host "Created test certificate $($cert.Thumbprint) in $signing (not imported anywhere)"
    }
    $password = Get-Content -LiteralPath $secret -Raw
    # The DLL first: the catalog hashes the signed file.
    Invoke-Tool "$bin\x64\signtool.exe" @('sign', '/q', '/fd', 'SHA256', '/f', $pfx, '/p', $password, "$package\rubylight_vgamepad.dll")
}

Invoke-Tool "$bin\x86\Inf2Cat.exe" @("/driver:$package", '/os:10_VB_X64,10_CO_X64,10_NI_X64,10_GE_X64', '/uselocaltime')

if ($TestSign) {
    Invoke-Tool "$bin\x64\signtool.exe" @('sign', '/q', '/fd', 'SHA256', '/f', $pfx, '/p', $password, "$package\rubylight_vgamepad.cat")
    $expected = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new($cer).Thumbprint
    foreach ($file in 'rubylight_vgamepad.dll', 'rubylight_vgamepad.cat') {
        $signature = Get-AuthenticodeSignature -LiteralPath "$package\$file"
        if (-not $signature.SignerCertificate -or $signature.SignerCertificate.Thumbprint -ne $expected) {
            throw "$file is not signed by the test certificate"
        }
        if ($signature.Status -eq 'HashMismatch' -or $signature.Status -eq 'NotSigned') {
            throw "$file signature is invalid: $($signature.StatusMessage)"
        }
        Write-Host "$file signed by $expected ($($signature.Status): $($signature.StatusMessage))"
    }
}

Get-ChildItem $package | Format-Table Name, Length
Write-Host "Driver package: $package"
