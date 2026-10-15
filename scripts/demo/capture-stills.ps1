# -Locale de captures the German stills into docs/screenshots/demo-de (per-app language, reset to English afterwards).
# -Theme picks the system theme the app follows (dark by default, as in the published stills); the prior mode is restored.
param([string[]]$Only, [string]$Locale = 'en', [ValidateSet('dark', 'light')][string]$Theme = 'dark')
. (Join-Path $PSScriptRoot 'capture-common.ps1')
$output = Join-Path $root $(if ($Locale -eq 'en') { 'docs/screenshots/demo' } else { "docs/screenshots/demo-$Locale" })
New-Item -ItemType Directory -Force $output | Out-Null
$states = [ordered]@{
    'pc-list' = 'hosts'; 'library' = 'library'; 'stream' = 'stream';
    'stream-compact' = 'compact'; 'stream-advanced' = 'advanced';
    'settings-presets' = 'settings'; 'upscaling-options' = 'upscaling';
    'controller' = 'controller'; 'touch-controller' = 'touch'; 'pip' = 'pip'; 'pc-settings' = 'pc-settings'
    'video-codecs' = 'codecs'; 'gpu-driver' = 'gpu-driver'; 'usb-controllers' = 'usb'
}
try {
    Initialize-Capture $Theme
    Set-CaptureLocale $Locale
    # A plain dark home screen for the picture-in-picture still (enabling it launches an activity, so do it before any stream).
    Enable-DemoHome
    foreach ($shot in $states.GetEnumerator()) {
        if ($Only -and $shot.Key -notin $Only) { continue }
        Open-DemoState $shot.Value
        if ($shot.Value -eq 'pip') { Enter-DemoPip }
        Start-Sleep -Milliseconds 800
        $path = Join-Path $output "$($shot.Key).png"
        $null = Invoke-Adb @('shell', 'screencap', '-p', "$remote/still.png")
        $null = Invoke-Adb @('pull', "$remote/still.png", $path)
        $bytes = [IO.File]::ReadAllBytes($path)
        $width = ($bytes[16] * 16777216) + ($bytes[17] * 65536) + ($bytes[18] * 256) + $bytes[19]
        $height = ($bytes[20] * 16777216) + ($bytes[21] * 65536) + ($bytes[22] * 256) + $bytes[23]
        if ($width -ne 1080 -or $height -ne 2400) { throw "$path is ${width}x${height}, expected 1080x2400." }
        Write-Host "Saved $path"
    }
    # Landscape stream stills: the sample fills the screen and the real on-screen controller is laid out for it.
    Set-Orientation $true
    foreach ($shot in ([ordered]@{ 'landscape-stream' = 'stream'; 'landscape-compact' = 'compact';
            'landscape-advanced' = 'advanced'; 'landscape-touch' = 'touch'; 'landscape-stream-menu' = 'menu'
            'landscape-statistics' = 'statistics' }).GetEnumerator()) {
        if ($Only -and $shot.Key -notin $Only) { continue }
        Open-DemoState $(if ($shot.Value -eq 'menu') { 'compact' } else { $shot.Value })
        # The stream menu scrolled to its Quality and Host sections, as in the video's stream menu shot.
        if ($shot.Value -eq 'menu') { Open-StreamMenu; Show-StreamMenuSections }
        Start-Sleep -Milliseconds 3500
        $path = Join-Path $output "$($shot.Key).png"
        $null = Invoke-Adb @('shell', 'screencap', '-p', "$remote/still.png")
        $null = Invoke-Adb @('pull', "$remote/still.png", $path)
        $bytes = [IO.File]::ReadAllBytes($path)
        $width = ($bytes[16] * 16777216) + ($bytes[17] * 65536) + ($bytes[18] * 256) + $bytes[19]
        $height = ($bytes[20] * 16777216) + ($bytes[21] * 65536) + ($bytes[22] * 256) + $bytes[23]
        if ($width -ne 2400 -or $height -ne 1080) { throw "$path is ${width}x${height}, expected 2400x1080." }
        Write-Host "Saved $path"
    }
    $null = Invoke-Adb @('shell', 'rm', "$remote/still.png")
} finally {
    if ($Locale -ne 'en') { Set-CaptureLocale 'en' }
    Restore-Capture
}
