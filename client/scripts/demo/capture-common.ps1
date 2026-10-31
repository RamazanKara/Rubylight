$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$package = 'com.butterpollo.client.debug'
$component = "$package/com.limelight.demo.DemoLauncherActivity"
$remote = '/sdcard/nightjar-demo'
$work = Join-Path $root 'out/demo/capture'
New-Item -ItemType Directory -Force $work | Out-Null
$command = Get-Command adb.exe -ErrorAction SilentlyContinue
$adb = if ($command) { $command.Source } else {
    $sdk = $env:ANDROID_HOME
    if (-not $sdk) { $sdk = 'C:/Android/sdk' }
    if (-not (Test-Path "$sdk/platform-tools/adb.exe")) { $sdk = "$env:LOCALAPPDATA/Android/Sdk" }
    Join-Path $sdk 'platform-tools/adb.exe'
}
if (-not (Test-Path -LiteralPath $adb)) { throw 'Put Android platform-tools on PATH.' }

function Invoke-Adb([string[]]$Arguments) {
    $ErrorActionPreference = 'Continue'
    $result = & $adb @Arguments 2>&1
    if ($LASTEXITCODE -ne 0) { throw "adb $($Arguments -join ' ') failed: $result" }
    return ($result -join "`n")
}

function Find-FFmpeg {
    $command = Get-Command ffmpeg.exe -ErrorAction SilentlyContinue
    if ($command) { return $command.Source }
    $winget = Join-Path $env:LOCALAPPDATA 'Microsoft/WinGet'
    $binary = Get-ChildItem -Path (Join-Path $winget 'Packages/Gyan.FFmpeg_*/*/bin/ffmpeg.exe') -ErrorAction SilentlyContinue |
        Sort-Object FullName | Select-Object -Last 1
    if ($binary) { return $binary.FullName }
    throw 'FFmpeg is not accessible. Open a new shell with the installed FFmpeg bin directory on PATH.'
}

# Labels the capture looks up that differ per app language (Set-CaptureLocale switches them).
$script:labels = @{ 'Presets' = 'Presets'; 'Native preset' = 'Native (recommended)'; 'Display & image' = 'Display & image'
    'Client-side upscaling' = 'Client-side upscaling'; 'Display on the PC' = 'Display on the PC'; 'Video' = 'Video'
    'Video codec' = 'Video codec'; 'Controls' = 'Controls'; 'Advanced' = 'Advanced'; 'GPU driver' = 'Snapdragon GPU driver'
    'PC display mode' = 'PC display mode'; 'USB controllers' = 'USB controllers'
    'More display' = 'More Display & image settings'; 'Sharp FSR' = 'Sharp (FSR)' }

function Set-CaptureLocale([string]$Locale) {
    # Per-app language (Android 13+), so the system UI and launcher stay as they are.
    $null = Invoke-Adb @('shell', 'cmd', 'locale', 'set-app-locales', $package, '--user', '0', '--locales', $Locale)
    $script:labels = switch ($Locale) {
        'de' { @{ 'Presets' = 'Vorgaben'; 'Native preset' = 'Nativ (empfohlen)'; 'Display & image' = 'Display und Bild'
            'Client-side upscaling' = 'Hochskalierung auf dem Gerät'; 'Display on the PC' = 'Anzeige am PC'; 'Video' = 'Video'
            'Video codec' = 'Videocodec'; 'Controls' = 'Steuerung'; 'Advanced' = 'Erweitert'; 'GPU driver' = 'Snapdragon-GPU-Treiber'
            'PC display mode' = 'PC-Displaymodus'; 'USB controllers' = 'USB-Controller'
            'More display' = 'Weitere Einstellungen für Display und Bild'; 'Sharp FSR' = 'Scharf (FSR)' } }
        default { @{ 'Presets' = 'Presets'; 'Native preset' = 'Native (recommended)'; 'Display & image' = 'Display & image'
            'Client-side upscaling' = 'Client-side upscaling'; 'Display on the PC' = 'Display on the PC'; 'Video' = 'Video'
            'Video codec' = 'Video codec'; 'Controls' = 'Controls'; 'Advanced' = 'Advanced'; 'GPU driver' = 'Snapdragon GPU driver'
            'PC display mode' = 'PC display mode'; 'USB controllers' = 'USB controllers'
            'More display' = 'More Display & image settings'; 'Sharp FSR' = 'Sharp (FSR)' } }
    }
}

function Set-CaptureTheme([string]$Theme) {
    # The app follows the system theme. Dark shows the Nightjar night surfaces and amber accents; light the day palette.
    if ($null -eq $script:oldNightMode) {
        $current = Invoke-Adb @('shell', 'cmd', 'uimode', 'night')
        $script:oldNightMode = if ($current -match 'Night mode: (\w+)') { $Matches[1] } else { 'auto' }
    }
    $null = Invoke-Adb @('shell', 'cmd', 'uimode', 'night', $(if ($Theme -eq 'light') { 'no' } else { 'yes' }))
}

function Get-Ui {
    $null = Invoke-Adb @('shell', 'uiautomator', 'dump', '--compressed', "$remote/ui.xml")
    # Pull the dump as bytes: piping it through the console would depend on the shell's output encoding.
    $local = Join-Path $work 'last-ui.xml'
    $null = Invoke-Adb @('pull', "$remote/ui.xml", $local)
    return [xml][IO.File]::ReadAllText($local, [Text.Encoding]::UTF8)
}

function Get-Bounds($Node) {
    return @([regex]::Matches($Node.GetAttribute('bounds'), '\d+') | ForEach-Object { [int]$_.Value })
}

# -Prefix matches a row whose text starts with the label, such as a section's More row with its count.
function Find-Ui([string]$Label, [switch]$Scroll, [switch]$Prefix) {
    for ($attempt = 0; $attempt -lt 12; $attempt++) {
        $tree = Get-Ui
        $cling = $tree.SelectSingleNode('//node[@resource-id="android:id/immersive_cling_title"]')
        if ($cling) {
            # Android shows this one-time confirmation the first time the stream enters immersive mode.
            $ok = $tree.SelectSingleNode('//node[@resource-id="android:id/ok"]')
            if ($ok) { Tap-Node $ok; Start-Sleep -Milliseconds 600; continue }
        }
        foreach ($node in $tree.SelectNodes('//node')) {
            $description = $node.GetAttribute('content-desc')
            if ($Label -in @($node.GetAttribute('text').Split("`n")[0], $description, $node.GetAttribute('resource-id')) -or
                $description.StartsWith("$Label,") -or ($Prefix -and $node.GetAttribute('text').StartsWith($Label))) {
                return $node
            }
        }
        if ($Scroll) {
            # Only scroll the app's own list: right after launch the dump can still show the launcher.
            $area = $tree.SelectSingleNode("//node[@scrollable='true' and @package='$package']")
            if ($area) {
                $b = Get-Bounds $area
                $x = [int](($b[0] + $b[2]) / 2)
                $y1 = [int]($b[1] + ($b[3] - $b[1]) * 0.8)
                $y2 = [int]($b[1] + ($b[3] - $b[1]) * 0.3)
                $null = Invoke-Adb @('shell', 'input', 'swipe', "$x", "$y1", "$x", "$y2", '300')
            }
        }
        Start-Sleep -Milliseconds 400
    }
    throw "UI '$Label' missing; check the app language and inspect out/demo/capture/last-ui.xml."
}

function Tap-Node($Node) {
    $b = Get-Bounds $Node
    $x = [int](($b[0] + $b[2]) / 2)
    $y = [int](($b[1] + $b[3]) / 2)
    $null = Invoke-Adb @('shell', 'input', 'tap', "$x", "$y")
}

function Tap-Ui([string]$Label, [switch]$Scroll, [switch]$Prefix) {
    Tap-Node (Find-Ui $Label -Scroll:$Scroll -Prefix:$Prefix)
}

function Hide-Ime([switch]$WaitForShow) {
    # The emulator's virtual hardware keyboard makes Android report an open input method (keyboard buttons in the
    # navigation bar) over the stream; real phones do not. Back closes only that. With -WaitForShow, poll for it first.
    for ($attempt = 0; $attempt -lt $(if ($WaitForShow) { 10 } else { 3 }); $attempt++) {
        $ime = Invoke-Adb @('shell', 'dumpsys', 'input_method')
        if ($ime -match 'mInputShown=true') {
            $null = Invoke-Adb @('shell', 'input', 'keyevent', 'KEYCODE_BACK')
            Start-Sleep -Milliseconds 900
            return
        }
        if (-not $WaitForShow) { return }
        Start-Sleep -Milliseconds 200
    }
}

# States beyond the launcher's own: settings sections and dialogs reached through the real settings UI, and the stream
# with its menu open.
$script:settingsSections = @{ 'settings-video' = 'Video'; 'codecs' = 'Video'; 'settings-display' = 'Display & image'
    'upscaling' = 'Display & image'; 'settings-controls' = 'Controls'; 'usb' = 'Controls'; 'settings-advanced' = 'Advanced' }

function Open-DemoState([string]$State) {
    $launchState = if ($script:settingsSections.ContainsKey($State)) { 'settings' } elseif ($State -eq 'menu') { 'compact' }
        else { $State }
    $null = Invoke-Adb @('shell', 'am', 'force-stop', $package)
    # Key events (Back, D-pad) switch Android out of touch mode, which draws focus rings on the next screen; a tap on empty
    # launcher wallpaper switches it back.
    $null = Invoke-Adb @('shell', 'input', 'tap', '540', '1000')
    Start-Sleep -Milliseconds 300
    $result = Invoke-Adb @('shell', 'am', 'start', '-W', '-n', $component, '--es', 'state', $launchState, '--es', 'codec', 'h264')
    if ($result -match 'Error|Exception') { throw "Install the demo debug APK first: $result" }
    $null = Invoke-Adb @('shell', 'wm', 'user-rotation', 'lock', $(if ($script:landscape) { '1' } else { '0' }))
    Start-Sleep -Milliseconds 1500
    switch ($State) {
        'hosts' { $null = Find-Ui 'Living-room PC' }
        # In landscape the running game is in the second row of the grid: scroll it into view.
        'library' { $null = Find-Ui 'Racing Game' -Scroll }
        'settings' {
            # Apply the recommended preset (it asks first), so the still shows a selected preset, not "Custom". The demo
            # preferences are written again on the next launch, so later states keep their FSR 1 demo values.
            $null = Find-Ui $script:labels['Presets']
            Tap-Ui $script:labels['Native preset']
            Tap-Ui 'android:id/button1'
            $null = Find-Ui $script:labels['Presets']
            Start-Sleep -Milliseconds 700
        }
        'controller' { $null = Find-Ui 'Demo wireless controller' }
        # Living-room PC's settings: the Save bar is at the bottom of the screen whatever the app language.
        'pc-settings' { $null = Find-Ui "${package}:id/host_profile_save"; Start-Sleep -Milliseconds 700 }
        { $script:settingsSections.ContainsKey($_) } {
            $null = Find-Ui $script:labels['Presets']
            Tap-Ui $script:labels[$script:settingsSections[$State]] -Scroll
            Start-Sleep -Milliseconds 900
            switch ($State) {
                'codecs' { Tap-Ui $script:labels['Video codec'] -Scroll; $null = Find-Ui 'PyroWave (host support required)' }
                # Client-side upscaling sits behind the section's More row (settings tiers): unfold it first.
                'upscaling' {
                    Tap-Ui $script:labels['More display'] -Scroll -Prefix
                    Start-Sleep -Milliseconds 600
                    Tap-Ui $script:labels['Client-side upscaling'] -Scroll; $null = Find-Ui $script:labels['Sharp FSR']
                }
                'usb' { Show-UsbControllers }
            }
            Start-Sleep -Milliseconds 600
        }
        'gpu-driver' { $null = Find-Ui $script:labels['GPU driver']; Start-Sleep -Milliseconds 600 }
        default {
            $dot = [string][char]0x00B7
            $null = Find-Ui "DEMO $dot local sample $dot scripted stats"
            Hide-Ime
            Start-Sleep -Seconds 2
            if ($State -eq 'menu') { Open-StreamMenu; Show-StreamMenuSections; Start-Sleep -Milliseconds 800 }
        }
    }
}

function Set-Orientation([bool]$Landscape) {
    # The stream screen fills the width in landscape. Open-DemoState re-applies the lock once the app is in front:
    # the launcher is portrait-only, so the window manager ignores a lock issued while it is on screen.
    $script:landscape = $Landscape
    $null = Invoke-Adb @('shell', 'wm', 'user-rotation', 'lock', $(if ($Landscape) { '1' } else { '0' }))
    Start-Sleep -Seconds 1
}

function Enable-DemoHome {
    # The emulator's launcher is full of third-party icons. For the PiP capture the debug app offers a plain dark
    # home screen (DemoHomeActivity, disabled in the manifest and enabled by the app itself on request): enable it and make it the preferred home until
    # Restore-Capture puts the device's own launcher back.
    if ($script:demoHomeActive) { return }
    $resolved = Invoke-Adb @('shell', 'cmd', 'package', 'resolve-activity', '--brief', '-a', 'android.intent.action.MAIN',
        '-c', 'android.intent.category.HOME')
    $script:oldHome = @($resolved -split "`n" | ForEach-Object { $_.Trim() } | Where-Object { $_ -match '^[\w.]+/[\w.$]+$' }) | Select-Object -Last 1
    $demoHome = "$package/com.limelight.demo.DemoHomeActivity"
    $null = Invoke-Adb @('shell', 'am', 'start', '-W', '-n', $component, '--es', 'home', 'enable')
    # The component state change lands a moment after the app's launcher activity has run; retry until it is a valid home.
    for ($attempt = 0; $attempt -lt 10; $attempt++) {
        Start-Sleep -Milliseconds 700
        $listed = Invoke-Adb @('shell', 'cmd', 'package', 'query-activities', '--brief', '-a', 'android.intent.action.MAIN',
            '-c', 'android.intent.category.HOME')
        if ($listed -match 'DemoHomeActivity') { break }
    }
    $script:demoHomeActive = $true  # from here on Restore-Capture undoes it, even if the next call fails
    for ($attempt = 0; $attempt -lt 10; $attempt++) {
        try {
            $null = Invoke-Adb @('shell', 'cmd', 'package', 'set-home-activity', $demoHome)
            return
        } catch {
            $failure = $_.Exception.Message
            Start-Sleep -Milliseconds 700
        }
    }
    throw "Could not make the demo home screen the preferred home: $failure"
}

function Disable-DemoHome {
    if (-not $script:demoHomeActive) { return }
    $script:demoHomeActive = $false
    if ($script:oldHome) { $null = Invoke-Adb @('shell', 'cmd', 'package', 'set-home-activity', $script:oldHome) }
    $null = Invoke-Adb @('shell', 'am', 'start', '-W', '-n', $component, '--es', 'home', 'disable')
}

function Enter-DemoPip {
    Enable-DemoHome
    $null = Invoke-Adb @('shell', 'input', 'keyevent', 'KEYCODE_HOME')
    for ($attempt = 0; $attempt -lt 10; $attempt++) {
        Start-Sleep -Milliseconds 500
        $activity = Invoke-Adb @('shell', 'dumpsys', 'activity', 'activities')
        if ($activity -match 'mode=pinned|windowingMode=2|mIsInPipMode=true') {
            # The window settles in Android's default corner (bottom right); the demo home screen's rings are centred
            # on that spot, so no dragging (which is timing dependent) is needed.
            Start-Sleep -Seconds 2
            return
        }
    }
    throw 'PiP did not open. Use an Android 8+ phone/emulator with PiP support enabled.'
}

function Open-StreamMenu {
    # Back opens the stream menu, as in a real stream. A Back can instead close the emulator's input method, so check.
    for ($attempt = 0; $attempt -lt 3; $attempt++) {
        $null = Invoke-Adb @('shell', 'input', 'keyevent', 'KEYCODE_BACK')
        Start-Sleep -Milliseconds 900
        if ((Get-Ui).SelectSingleNode("//node[@resource-id='${package}:id/action_sheet_close']")) {
            Start-Sleep -Milliseconds 500
            return
        }
    }
    throw 'The stream menu did not open; inspect out/demo/capture/last-ui.xml.'
}

function Show-StreamMenuSections {
    # From the top of the open menu (quick toggles, Input), two slow strokes scroll it to Quality (Set bitrate, Automatic
    # stream quality, Automatic bitrate) with Host below. The swipes are for the 2400x1080 landscape stream. Slow strokes
    # barely fling, so the sheet stops near the same place every take; one shell call keeps the strokes close together.
    $null = Invoke-Adb @('shell', 'input swipe 1260 1050 1260 250 800; input swipe 1260 1050 1260 250 800')
}

function Invoke-Fling([int]$From, [int]$To, [int]$Milliseconds = 300) {
    # A drag that ends moving, so the list flings and settles with Android's own deceleration. With the same stroke the
    # list stops within a few dozen pixels of the same place every take.
    $null = Invoke-Adb @('shell', 'input', 'swipe', '540', "$From", '540', "$To", "$Milliseconds")
}

function Reset-ListToTop {
    # Two quick strokes down bring a settings list back to its first row.
    $null = Invoke-Adb @('shell', 'input swipe 540 700 540 2200 120; input swipe 540 700 540 2200 120')
    Start-Sleep -Milliseconds 1300
}

function Show-UsbControllers {
    # From the top of Controls, one fling brings the USB controllers group up under the toolbar.
    Invoke-Fling 2150 700 450
    Start-Sleep -Milliseconds 1800
}

function Initialize-Capture([string]$Theme = 'dark') {
    if ((Invoke-Adb @('get-state')).Trim() -ne 'device') { throw 'Connect one authorized device or set ANDROID_SERIAL.' }
    Set-CaptureTheme $Theme
    $script:oldSize = Invoke-Adb @('shell', 'wm', 'size')
    $script:oldDensity = Invoke-Adb @('shell', 'wm', 'density')
    $script:oldRotation = (Invoke-Adb @('shell', 'settings', 'get', 'system', 'user_rotation')).Trim()
    $script:oldAutoRotation = (Invoke-Adb @('shell', 'settings', 'get', 'system', 'accelerometer_rotation')).Trim()
    $null = Invoke-Adb @('shell', 'mkdir', '-p', $remote)
    $null = Invoke-Adb @('shell', 'wm', 'size', '1080x2400')
    $null = Invoke-Adb @('shell', 'wm', 'density', '420')
    $null = Invoke-Adb @('shell', 'settings', 'put', 'system', 'accelerometer_rotation', '0')
    $null = Invoke-Adb @('shell', 'settings', 'put', 'system', 'user_rotation', '0')
    $script:landscape = $false
    # The emulator reports a hardware keyboard but still pops up the soft keyboard over the stream; real phones do not.
    $script:oldImeSetting = (Invoke-Adb @('shell', 'settings', 'get', 'secure', 'show_ime_with_hard_keyboard')).Trim()
    $null = Invoke-Adb @('shell', 'settings', 'put', 'secure', 'show_ime_with_hard_keyboard', '0')
    $null = Invoke-Adb @('shell', 'wm', 'user-rotation', 'lock', '0')
    # Android confirms full screen the first time an app enters it ("Viewing full screen"), which would cover a stream.
    $script:oldImmersive = (Invoke-Adb @('shell', 'settings', 'get', 'secure', 'immersive_mode_confirmations')).Trim()
    $null = Invoke-Adb @('shell', 'settings', 'put', 'secure', 'immersive_mode_confirmations', 'confirmed')
    Enter-CleanStatusBar
}

function Enter-CleanStatusBar {
    # SystemUI demo mode: fixed clock, full battery and Wi-Fi, no mobile signal or notification icons.
    $script:oldDemoAllowed = (Invoke-Adb @('shell', 'settings', 'get', 'global', 'sysui_demo_allowed')).Trim()
    $null = Invoke-Adb @('shell', 'settings', 'put', 'global', 'sysui_demo_allowed', '1')
    foreach ($demo in @(@('enter'), @('clock', '-e', 'hhmm', '1030'), @('battery', '-e', 'level', '100', '-e', 'plugged', 'false'),
            @('network', '-e', 'wifi', 'show', '-e', 'level', '4', '-e', 'fully', 'true'),
            # Android 15 labels a shown mobile signal "3G" whatever the datatype; Wi-Fi alone reads cleaner.
            @('network', '-e', 'mobile', 'hide'),
            @('notifications', '-e', 'visible', 'false'))) {
        $null = Invoke-Adb (@('shell', 'am', 'broadcast', '-a', 'com.android.systemui.demo', '-e', 'command') + $demo)
    }
}

function Exit-CleanStatusBar {
    if ($null -eq $script:oldDemoAllowed) { return }
    $null = Invoke-Adb @('shell', 'am', 'broadcast', '-a', 'com.android.systemui.demo', '-e', 'command', 'exit')
    if ($script:oldDemoAllowed -eq 'null') {
        $null = Invoke-Adb @('shell', 'settings', 'delete', 'global', 'sysui_demo_allowed')
    } else {
        $null = Invoke-Adb @('shell', 'settings', 'put', 'global', 'sysui_demo_allowed', $script:oldDemoAllowed)
    }
}

function Restore-Capture {
    Disable-DemoHome
    Exit-CleanStatusBar
    if ($null -ne $script:oldNightMode) { $null = Invoke-Adb @('shell', 'cmd', 'uimode', 'night', $script:oldNightMode) }
    if ($null -ne $script:oldImmersive) {
        if ($script:oldImmersive -eq 'null') { $null = Invoke-Adb @('shell', 'settings', 'delete', 'secure', 'immersive_mode_confirmations') }
        else { $null = Invoke-Adb @('shell', 'settings', 'put', 'secure', 'immersive_mode_confirmations', $script:oldImmersive) }
    }
    if ($null -ne $script:oldImeSetting) {
        if ($script:oldImeSetting -eq 'null') { $null = Invoke-Adb @('shell', 'settings', 'delete', 'secure', 'show_ime_with_hard_keyboard') }
        else { $null = Invoke-Adb @('shell', 'settings', 'put', 'secure', 'show_ime_with_hard_keyboard', $script:oldImeSetting) }
    }
    if (-not $script:oldSize) { return }
    $null = Invoke-Adb @('shell', 'am', 'force-stop', $package)
    $size = if ($script:oldSize -match 'Override size: (\d+x\d+)') { $Matches[1] } else { 'reset' }
    $density = if ($script:oldDensity -match 'Override density: (\d+)') { $Matches[1] } else { 'reset' }
    $null = Invoke-Adb @('shell', 'wm', 'size', $size)
    $null = Invoke-Adb @('shell', 'wm', 'density', $density)
    foreach ($setting in @(@('user_rotation', $script:oldRotation), @('accelerometer_rotation', $script:oldAutoRotation))) {
        if ($setting[1] -eq 'null') {
            $null = Invoke-Adb @('shell', 'settings', 'delete', 'system', $setting[0])
        } else {
            $null = Invoke-Adb @('shell', 'settings', 'put', 'system', $setting[0], $setting[1])
        }
    }
    # wm keeps its own lock after Set-Orientation; hand rotation back to the device's prior mode.
    if ($script:oldAutoRotation -eq '1') { $null = Invoke-Adb @('shell', 'wm', 'user-rotation', 'free') }
    else { $null = Invoke-Adb @('shell', 'wm', 'user-rotation', 'lock', $(if ($script:oldRotation -match '^\d$') { $script:oldRotation } else { '0' })) }
}
