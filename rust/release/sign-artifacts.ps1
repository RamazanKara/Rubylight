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
    'butterpollo_truehdr.dll', 'vulkan-layer/butterpollo_vulkan_layer.dll'
)
# Rubylight's own display driver package, once rust/build.ps1 adds one: an
# INF and its DLL. The DLL is signed with the binaries, the catalog is made
# from the signed DLL, then the "driver" request signs the catalog
# (.signpath/driver.xml). Packages without the folder skip all of this.
$driver = Join-Path $distribution 'drivers\rubylight-vdisplay'
function Get-DriverFile([string]$Directory, [string]$Extension) {
    $files = @(Get-ChildItem -LiteralPath $Directory -File -Filter "*$Extension")
    if ($files.Count -ne 1) { throw "Expected one $Extension file in $Directory, found $($files.Count)" }
    $files[0]
}
function Get-Release {
    # The first-party binaries this package ships, relative to the distribution.
    $names = @($binaries)
    if (Test-Path -LiteralPath $driver) { $names += 'drivers/rubylight-vdisplay/' + (Get-DriverFile $driver '.dll').Name }
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
} elseif ($Phase -eq 'StageDriver') {
    # Microsoft's order: sign the DLL, make the catalog, sign the catalog. No
    # signature then changes a file after the catalog has hashed it.
    $inf = Get-DriverFile $driver '.inf'
    $dll = Get-DriverFile $driver '.dll'
    Assert-Signed $dll.FullName
    Get-ChildItem -LiteralPath $driver -File -Filter '*.cat' | Remove-Item -Force
    $inf2cat = Get-KitTool 'Inf2Cat.exe' 'x86'
    & $inf2cat "/driver:$driver" '/os:10_X64' '/uselocaltime' | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "Inf2Cat failed with exit code $LASTEXITCODE" }
    $catalog = Get-DriverFile $driver '.cat'
    $named = "^\s*CatalogFile(\.\w+)?\s*=\s*$([regex]::Escape($catalog.Name))\s*(;.*)?$"
    if (!((Get-Content -LiteralPath $inf.FullName) -match $named)) { throw "$($inf.Name) does not name the catalog $($catalog.Name)" }
    New-Item -ItemType Directory -Path $SigningDirectory -Force | Out-Null
    foreach ($file in $inf, $dll, $catalog) { Copy-Item -LiteralPath $file.FullName -Destination $SigningDirectory }
} elseif ($Phase -eq 'ApplyDriver') {
    $catalog = Get-DriverFile $SigningDirectory '.cat'
    Assert-Signed $catalog.FullName
    # The signed catalog must still cover the packaged INF and signed DLL.
    $signtool = Get-KitTool 'signtool.exe' 'x64'
    foreach ($extension in '.inf', '.dll') {
        $file = (Get-DriverFile $driver $extension).FullName
        $output = & $signtool verify /pa /c $catalog.FullName $file 2>&1
        if ($LASTEXITCODE -ne 0) { throw "The signed catalog does not cover $file`n$($output -join "`n")" }
    }
    Copy-Item -LiteralPath $catalog.FullName -Destination $driver -Force
} else {
    $name = "butterpollo-setup-$Version.exe"
    $source = Join-Path $SigningDirectory $name
    Assert-Signed $source
    Copy-Item -LiteralPath $source -Destination (Join-Path $TargetDirectory $name) -Force
}
