# Captures the Google Play Android TV screenshots from the demo-mode debug build on an Android TV device or emulator
# at 1920x1080 (the nj-tv AVD in docs/demo/README.md), straight into fastlane tvScreenshots. Play wants 16:9 JPEG or
# 24-bit PNG without alpha, so each capture is redrawn without its alpha channel.
# Usage: $env:ANDROID_SERIAL = 'emulator-5554'; pwsh scripts/store/tv-screenshots.ps1 [-Only home,detail]
param([string[]]$Only)
. (Join-Path $PSScriptRoot '../demo/capture-common.ps1')
Add-Type -AssemblyName System.Drawing
$output = Join-Path $root 'fastlane/metadata/android/en-US/images/tvScreenshots'
New-Item -ItemType Directory -Force $output | Out-Null

# (file, launcher extras, label that shows the screen is ready). All screens use the Rubylight-style demo library
# (host native) so Home has its store shelves.
$shots = @(
    @{ File = '1_home.png'; Extras = @('--es', 'state', 'home'); Ready = 'Living-room PC' },
    @{ File = '2_game_page.png'; Extras = @('--es', 'state', 'detail', '--ei', 'app', '2'); Ready = 'Living-room PC' },
    @{ File = '3_stream.png'; Extras = @('--es', 'state', 'compact'); Ready = "DEMO $([char]0x00B7) local sample $([char]0x00B7) scripted stats" },
    @{ File = '4_all_games.png'; Extras = @('--es', 'state', 'allgames'); Ready = 'Living-room PC' }
)

if ((Invoke-Adb @('get-state')).Trim() -ne 'device') { throw 'Connect one authorized Android TV device or set ANDROID_SERIAL.' }
if ((Invoke-Adb @('shell', 'pm', 'has-feature', 'android.software.leanback')).Trim() -ne 'true') { throw 'The device is not an Android TV.' }
$size = Invoke-Adb @('shell', 'wm', 'size')
if ($size -notmatch '1920x1080') { throw "The TV must be 1920x1080: $size" }
$oldImmersive = (Invoke-Adb @('shell', 'settings', 'get', 'secure', 'immersive_mode_confirmations')).Trim()
$null = Invoke-Adb @('shell', 'settings', 'put', 'secure', 'immersive_mode_confirmations', 'confirmed')
$null = Invoke-Adb @('shell', 'mkdir', '-p', $remote)
try {
    foreach ($shot in $shots) {
        if ($Only -and $shot.Extras[2] -notin $Only) { continue }
        $null = Invoke-Adb @('shell', 'am', 'force-stop', $package)
        $result = Invoke-Adb (@('shell', 'am', 'start', '-W', '-n', $component, '--es', 'ui', 'tv', '--es', 'host', 'native',
                '--es', 'codec', 'h264') + $shot.Extras)
        if ($result -match 'Error|Exception') { throw "Install the demo debug APK first: $result" }
        $null = Find-Ui $shot.Ready
        Hide-Ime
        Start-Sleep -Seconds 2
        $raw = Join-Path $work "tv-$($shot.File)"
        $null = Invoke-Adb @('shell', 'screencap', '-p', "$remote/tv.png")
        $null = Invoke-Adb @('pull', "$remote/tv.png", $raw)
        $source = [Drawing.Image]::FromFile($raw)
        try {
            if ($source.Width -ne 1920 -or $source.Height -ne 1080) { throw "$raw is $($source.Width)x$($source.Height), expected 1920x1080." }
            $opaque = New-Object Drawing.Bitmap 1920, 1080, ([Drawing.Imaging.PixelFormat]::Format24bppRgb)
            $graphics = [Drawing.Graphics]::FromImage($opaque)
            $graphics.DrawImage($source, 0, 0, 1920, 1080)
            $graphics.Dispose()
            $path = Join-Path $output $shot.File
            $opaque.Save($path, [Drawing.Imaging.ImageFormat]::Png)
            $opaque.Dispose()
        } finally { $source.Dispose() }
        Write-Host "Saved $path"
    }
} finally {
    $null = Invoke-Adb @('shell', 'am', 'force-stop', $package)
    if ($oldImmersive -eq 'null') { $null = Invoke-Adb @('shell', 'settings', 'delete', 'secure', 'immersive_mode_confirmations') }
    else { $null = Invoke-Adb @('shell', 'settings', 'put', 'secure', 'immersive_mode_confirmations', $oldImmersive) }
}
