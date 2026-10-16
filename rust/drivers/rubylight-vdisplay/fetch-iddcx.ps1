# Fetches IddCx 1.11's headers and IddCxStub.lib into target\iddcx-1.11, from the pinned
# Windows Driver Kit NuGet package. The installed WDK 10.0.26100 stops at IddCx 1.10; 1.11
# (the D3D12 swap chain) ships in WDK 10.0.28000. Only these four files are taken from it:
# everything else still builds against the installed kit.
#
# Dot-sourced by driver-env.ps1, which sets RLVD_IDDCX_ROOT to the result. Does nothing when
# the files are already there.
$ErrorActionPreference = 'Stop'
$iddcxRoot = Join-Path $PSScriptRoot 'target\iddcx-1.11'
$iddcxFiles = @(
    @{ From = 'c/Include/10.0.28000.0/um/iddcx/1.11/IddCx.h'; To = 'include\IddCx.h' },
    @{ From = 'c/Include/10.0.28000.0/um/iddcx/1.11/IddCxFuncEnum.h'; To = 'include\IddCxFuncEnum.h' },
    @{ From = 'c/Include/10.0.28000.0/um/iddcx/1.11/IddCxTraceEnums.h'; To = 'include\IddCxTraceEnums.h' },
    @{ From = 'c/Lib/10.0.28000.0/um/x64/iddcx/1.11/iddcxstub.lib'; To = 'lib\IddCxStub.lib' }
)
if (@($iddcxFiles | Where-Object { !(Test-Path -LiteralPath (Join-Path $iddcxRoot $_.To)) }).Count -gt 0) {
    $package = 'microsoft.windows.wdk.x64'
    $version = '10.0.28000.2526'
    $sha256 = '63C939FB5A79295BF40E941DB592681272219B04EDFF095FE2F3D123E5579A90'
    $archive = Join-Path $PSScriptRoot "target\$package.$version.nupkg"
    New-Item -ItemType Directory -Force (Split-Path $archive) | Out-Null
    if (!(Test-Path -LiteralPath $archive) -or (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ne $sha256) {
        $progress = $ProgressPreference
        $ProgressPreference = 'SilentlyContinue'
        try {
            Invoke-WebRequest -UseBasicParsing "https://api.nuget.org/v3-flatcontainer/$package/$version/$package.$version.nupkg" -OutFile $archive
        } finally {
            $ProgressPreference = $progress
        }
        $actual = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash
        if ($actual -ne $sha256) {
            Remove-Item -LiteralPath $archive
            throw "$package $version has SHA-256 $actual, expected $sha256"
        }
    }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [IO.Compression.ZipFile]::OpenRead($archive)
    try {
        foreach ($file in $iddcxFiles) {
            $entry = $zip.GetEntry($file.From)
            if (!$entry) { throw "$archive has no $($file.From)" }
            $to = Join-Path $iddcxRoot $file.To
            New-Item -ItemType Directory -Force (Split-Path $to) | Out-Null
            [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $to, $true)
        }
    } finally {
        $zip.Dispose()
    }
}
