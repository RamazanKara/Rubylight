param([string[]]$Shot, [switch]$NoHost)

# All current shotlist states are offline fixtures; retain the original entry point.
& (Join-Path $PSScriptRoot 'capture-video.ps1') -Shot $Shot
