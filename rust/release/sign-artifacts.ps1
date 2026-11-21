param(
    [Parameter(Mandatory)][ValidateSet('CheckMetadata', 'StageBinaries', 'ApplyBinaries', 'StageDriver', 'ApplyDriver', 'ApplyInstaller')][string]$Phase,
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
    'vulkan-layer/butterpollo_vulkan_layer.dll'
)
# Rubylight's own display driver package (rust/build.ps1): an INF, its DLL
# and an unsigned catalog, next to install-local.ps1 and nefconc.exe. The DLL
# is signed with the binaries, the catalog is made again from the signed DLL,
# then the "driver" request signs it (.signpath/driver.xml); install-local.ps1
# installs a signed catalog as it is. Packages without the folder skip this.
$driver = Join-Path $distribution 'drivers\rubylight-display'
$gamepadDriver = Join-Path $distribution 'drivers\rubylight-gamepad'
$drivers = @(
    @{ Path = $driver; Name = 'drivers/rubylight-display'; Os = '10_X64' },
    @{ Path = $gamepadDriver; Name = 'drivers/rubylight-gamepad'; Os = '10_VB_X64,10_CO_X64,10_NI_X64,10_GE_X64' }
)
function Get-DriverFile([string]$Directory, [string]$Extension) {
    $files = @(Get-ChildItem -LiteralPath $Directory -File -Filter "*$Extension")
    if ($files.Count -ne 1) { throw "Expected one $Extension file in $Directory, found $($files.Count)" }
    $files[0]
}
function Get-CatalogName([IO.FileInfo]$Inf) {
    foreach ($line in Get-Content -LiteralPath $Inf.FullName) {
        if ($line -match '^\s*CatalogFile(?:\.\w+)?\s*=\s*([^\s;]+)') { return $Matches[1] }
    }
    throw "$($Inf.Name) does not declare CatalogFile"
}
function Get-Release {
    # The first-party binaries this package ships, relative to the distribution.
    $names = @($binaries)
    foreach ($d in $drivers) {
        if (Test-Path -LiteralPath $d.Path) { $names += "$($d.Name)/" + (Get-DriverFile $d.Path '.dll').Name }
    }
    $names
}
function Assert-Signed([string]$Path) {
    $signature = Get-AuthenticodeSignature -LiteralPath $Path
    if ($signature.Status -ne 'Valid' -or !$signature.SignerCertificate -or !$signature.TimeStamperCertificate) {
        throw "Missing valid timestamped Authenticode signature: $Path ($($signature.Status))"
    }
}
# The service carries its own version, so that its file stays the same from
# release to release (rust/service); binaries.xml signs it under that version.
$serviceVersion = (Select-String -LiteralPath (Join-Path $PSScriptRoot '..\service\Cargo.toml') -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
function Assert-Metadata([string]$Path) {
    # SignPath signs only files whose version information names the project
    # and the release: the restrictions in .signpath/*.xml.
    $expected = if ((Split-Path -Leaf $Path) -eq 'butterpollo-service.exe') { $serviceVersion } else { $Version }
    $info = (Get-Item -LiteralPath $Path).VersionInfo
    if ($info.ProductName -ne 'Rubylight' -or $info.ProductVersion -ne $expected) {
        throw "$Path has product '$($info.ProductName)' version '$($info.ProductVersion)', not Rubylight $expected"
    }
}
# A service the package took from the previous release, signed there
# (rust/release/previous_service.py), keeps that release's bytes: signing it
# again would change its file. It is not sent, which binaries.xml allows.
function Test-KeptSigned([string]$Name) {
    if ($Name -ne 'butterpollo-service.exe') { return $false }
    $signature = Get-AuthenticodeSignature -LiteralPath (Join-Path $distribution $Name)
    [bool]($signature.Status -eq 'Valid' -and $signature.SignerCertificate -and $signature.TimeStamperCertificate)
}
function Get-KitTool([string]$Name, [string]$Architecture) {
    $kits = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
    $tool = Get-ChildItem -Path (Join-Path $kits "*\$Architecture\$Name") -File -ErrorAction SilentlyContinue |
        Sort-Object { try { [version]$_.Directory.Parent.Name } catch { [version]'0.0' } } -Descending |
        Select-Object -First 1
    if (!$tool) { throw "$Name was not found under $kits; the Windows Driver Kit provides it" }
    $tool.FullName
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
        if (Test-KeptSigned $name) {
            Write-Host "$name keeps the previous release's signature"
            continue
        }
        $target = Join-Path $SigningDirectory $name
        New-Item -ItemType Directory -Path (Split-Path -Parent $target) -Force | Out-Null
        Copy-Item -LiteralPath $source -Destination $target
    }
} elseif ($Phase -eq 'ApplyBinaries') {
    # Verify the complete set before replacing anything. Third-party binaries
    # retain their upstream signatures and are never submitted for re-signing.
    $names = @(Get-Release | Where-Object { !(Test-KeptSigned $_) })
    foreach ($name in $names) { Assert-Signed (Join-Path $SigningDirectory $name) }
    foreach ($name in $names) {
        Copy-Item -LiteralPath (Join-Path $SigningDirectory $name) -Destination (Join-Path $distribution $name) -Force
    }
} elseif ($Phase -eq 'StageDriver') {
    # Microsoft's order: sign the DLL, make the catalog, sign the catalog. No
    # signature then changes a file after the catalog has hashed it.
    New-Item -ItemType Directory -Path $SigningDirectory -Force | Out-Null
    $inf2cat = Get-KitTool 'Inf2Cat.exe' 'x86'
    foreach ($d in $drivers) {
        if (!(Test-Path -LiteralPath $d.Path)) { continue }
        $inf = Get-DriverFile $d.Path '.inf'
        $dll = Get-DriverFile $d.Path '.dll'
        Assert-Signed $dll.FullName
        Get-ChildItem -LiteralPath $d.Path -File -Filter '*.cat' | Remove-Item -Force
        & $inf2cat "/driver:$($d.Path)" "/os:$($d.Os)" '/uselocaltime' | Out-Host
        if ($LASTEXITCODE -ne 0) { throw "Inf2Cat failed with exit code $LASTEXITCODE" }
        $catalog = Get-DriverFile $d.Path '.cat'
        if ((Get-CatalogName $inf) -ne $catalog.Name) { throw "$($inf.Name) does not name the catalog $($catalog.Name)" }
        foreach ($file in $inf, $dll, $catalog) { Copy-Item -LiteralPath $file.FullName -Destination $SigningDirectory }
    }
} elseif ($Phase -eq 'ApplyDriver') {
    $signtool = Get-KitTool 'signtool.exe' 'x64'
    foreach ($d in $drivers) {
        if (!(Test-Path -LiteralPath $d.Path)) { continue }
        $inf = Get-DriverFile $d.Path '.inf'
        $catalog = Join-Path $SigningDirectory (Get-CatalogName $inf)
        if (!(Test-Path -LiteralPath $catalog -PathType Leaf)) { throw "Missing signed catalog: $catalog" }
        Assert-Signed $catalog
        # The signed catalog must still cover the packaged INF and signed DLL.
        foreach ($extension in '.inf', '.dll') {
            $file = (Get-DriverFile $d.Path $extension).FullName
            $output = & $signtool verify /pa /c $catalog $file 2>&1
            if ($LASTEXITCODE -ne 0) { throw "The signed catalog does not cover $file`n$($output -join "`n")" }
        }
        Copy-Item -LiteralPath $catalog -Destination $d.Path -Force
    }
} else {
    $name = "butterpollo-setup-$Version.exe"
    $source = Join-Path $SigningDirectory $name
    Assert-Signed $source
    Copy-Item -LiteralPath $source -Destination (Join-Path $TargetDirectory $name) -Force
}
