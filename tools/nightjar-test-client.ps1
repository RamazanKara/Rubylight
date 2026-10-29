# Puts a Nightjar desktop build where test runs start it: one fixed folder per slot.
#   $exe = tools\nightjar-test-client.ps1 -From <nightjar-windows.zip | nightjar.exe> [-Slot a|b]
# Windows' firewall asks about each program path that opens a socket and keeps the answer per
# path, so a build started from a new folder every run asks every time. The same folder asks
# once: C:\tests\nightjar\<slot>\nightjar.exe on this PC, and on the laptop
# C:\tests\dn-nj\nightjar.exe (-Root C:\tests -Slot dn-nj), which is allowed there already.
param(
    [Parameter(Mandatory)][string]$From,
    [string]$Slot = 'a',
    [string]$Root = 'C:\tests\nightjar'
)
$ErrorActionPreference = 'Stop'
if ($Slot -notmatch '^[\w-]+$') { throw "slot '$Slot' is not a folder name" }
$dir = Join-Path $Root $Slot
$running = Get-Process nightjar -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path.StartsWith("$dir\", 'OrdinalIgnoreCase') }
if ($running) { throw "nightjar.exe is running from $dir (pid $($running.Id -join ', ')); stop that run first" }
if (Test-Path $dir) { Remove-Item -Recurse -Force $dir }
New-Item -ItemType Directory -Force $dir | Out-Null
if ($From -like '*.zip') {
    Expand-Archive -Path $From -DestinationPath $dir
} else {
    # A cargo build: the exe and the DLLs beside it (SDL, the PyroWave SDK).
    $source = Split-Path -Parent (Resolve-Path $From)
    Copy-Item (Resolve-Path $From) (Join-Path $dir 'nightjar.exe')
    Get-ChildItem $source -Filter *.dll | Copy-Item -Destination $dir
}
$exe = Join-Path $dir 'nightjar.exe'
if (-not (Test-Path $exe)) { throw "$From has no nightjar.exe" }
$exe
