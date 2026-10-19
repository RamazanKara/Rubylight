# Capture Windows' reason for refusing a virtual display mode, without changing
# the driver, display settings, registry, or Rubylight service.
# https://learn.microsoft.com/windows-hardware/drivers/display/indirect-display-debugging
[CmdletBinding()]
param(
    [ValidateRange(5, 300)]
    [int]$Seconds = 45,
    [string]$OutputDirectory = (Join-Path (Get-Location) ("rubylight-display-trace-" + (Get-Date -Format 'yyyyMMdd-HHmmss')))
)

$ErrorActionPreference = 'Stop'
$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
if (!$principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Run this script in an administrator PowerShell so Windows allows the IddCx trace.'
}
if (Test-Path -LiteralPath $OutputDirectory) {
    throw "Choose a new output directory; this one already exists: $OutputDirectory"
}
$directory = New-Item -ItemType Directory -Path $OutputDirectory
$trace = Join-Path $directory.FullName 'IddCx.etl'
$session = 'RubylightDisplay-' + [guid]::NewGuid().ToString('N')

# A private session name prevents stopping a trace started by somebody else.
# Bound the file size and omit high-frequency frame/cursor tracing.
& logman.exe create trace $session -o $trace -ets -ow -f bincirc -max 64 `
    -p '{D92BCB52-FA78-406F-A9A5-2037509FADEA}' 0x4f4 0xFF
if ($LASTEXITCODE -ne 0) { throw "Windows could not start the IddCx trace ($LASTEXITCODE)." }
try {
    Write-Host "Tracing for $Seconds seconds. Start the failing Moonlight stream now."
    $os = Get-CimInstance Win32_OperatingSystem
    [ordered]@{
        Windows = $os.Caption
        Version = $os.Version
        Build = $os.BuildNumber
        Graphics = @(Get-CimInstance Win32_VideoController | Select-Object Name, DriverVersion)
        TraceFlags = '0x4f4'
    } | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $directory.FullName 'environment.json') -Encoding utf8
    Start-Sleep -Seconds $Seconds
} finally {
    & logman.exe -stop $session -ets
    if ($LASTEXITCODE -ne 0) {
        Write-Warning "Windows could not stop trace $session ($LASTEXITCODE). Stop it with: logman -stop $session -ets"
    }
}
Write-Host "Trace saved in $($directory.FullName). Keep it with a support bundle from the same reproduction."
Write-Host 'The IddCx WPP messages require matching Microsoft public symbols to decode.'
