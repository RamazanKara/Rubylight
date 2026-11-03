# Scans release downloads with Microsoft Defender before a tag, report only:
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\defender-scan.ps1 <zip|exe>...
#
# A zip (the desktop-windows artifact, the release zip) is scanned as it is, archives inside it
# included, and never unpacked: real-time protection leaves a zip be but quarantines a flagged
# exe as soon as it is written. MpCmdRun runs with -DisableRemediation, so a hit is reported and
# nothing is removed; Defender's settings stay as they are. Exit 0 when clean, 2 on a threat,
# 1 when a scan could not run.
#
# What it catches: the PC's signatures and its own ML models, and every hash Microsoft has
# already flagged. What it cannot: the cloud's verdict on a first launch (Block at First Sight).
# Nightjar 4.4.0's first build scanned clean until it was started once; the cloud then called
# it Trojan:Win32/Bearfoos.A!ml and sent a signature for that exact file (FastPath), and a copy
# with one byte changed scanned clean again, and started clean (a new hash, the same code).
param([Parameter(Mandatory, ValueFromRemainingArguments)][string[]]$Path)
$ErrorActionPreference = 'Stop'

$mp = Join-Path $env:ProgramFiles 'Windows Defender\MpCmdRun.exe'
if (-not (Test-Path $mp)) { Write-Error "$mp is missing"; exit 1 }
$threat = $false
foreach ($item in $Path) {
    $file = (Resolve-Path -LiteralPath $item).Path
    $out = & $mp -Scan -ScanType 3 -File $file -DisableRemediation 2>&1 | Out-String
    $code = $LASTEXITCODE
    if ($code -eq 0 -and $out -match 'found no threats') {
        "clean   $file"
    } elseif ($out -match 'found \d+ threats') {
        "THREAT  $file"
        ($out -split "`r?`n" | Where-Object { $_ -match '^\s*(Threat|file)\s+:' }) -join "`n"
        $threat = $true
    } else {
        Write-Error "the scan of $file did not finish (exit $code): $($out.Trim())"
        exit 1
    }
}
if ($threat) { exit 2 }
exit 0
