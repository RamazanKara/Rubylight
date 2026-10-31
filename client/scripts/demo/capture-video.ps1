# Records the tour's shots from the debug demo as 60 fps clips (out/demo/clips/<shot-id>.mp4).
# -Shot retakes some shots by ID; a retake replaces a clip only when it missed fewer frames, unless -Replace is given.
# -Takes is how many times each shot is taken at most (the best take is kept). -MaxHostLoad waits, before each take, until
# the host's CPU load is below that percentage (up to five minutes): the emulator misses frames on a busy host.
# -HostCapture records the emulator's window on the PC (Desktop Duplication, encoded on the GPU with AMF) instead of
# with screenrecord inside the device; see docs/demo/README.md. -WindowHeight is the height of the window's display area.
# -Theme picks the system theme the app follows (dark by default, matching the stills); the prior mode is restored.
param([string[]]$Shot, [ValidateSet('dark', 'light')][string]$Theme = 'dark', [int]$Takes = 3, [int]$MaxHostLoad = 50,
    [switch]$Replace, [switch]$HostCapture, [int]$WindowHeight = 1320)
. (Join-Path $PSScriptRoot 'capture-common.ps1')
$output = Join-Path $root 'out/demo/clips'
New-Item -ItemType Directory -Force $output | Out-Null
$shots = @(Get-Content (Join-Path $root 'docs/demo/shotlist.md') | ForEach-Object {
    if ($_ -match '^\| \d{2}-') {
        $cells = $_.Split('|').Trim()
        [pscustomobject]@{ Id = $cells[1]; Seconds = [int]$cells[2]; State = $cells[8] }
    }
})
$ffmpeg = Find-FFmpeg
$ffprobe = Join-Path (Split-Path -Parent $ffmpeg) 'ffprobe.exe'
$fps = 60
# 0.6 of the 1080x2400 screen. The emulator's screen recorder keeps up with every 60 Hz refresh at this size (at the
# full size it manages about 30); the renders never show the phone larger than this.
$portraitSize = @(648, 1440)
if ($Shot) {
    foreach ($id in $Shot) {
        if ($id -notin $shots.Id) { throw "Unknown shot: $id" }
    }
    $shots = @($shots | Where-Object { $_.Id -in $Shot })
}

# ---- The takes ------------------------------------------------------------------------------------------------------
# Each take is planned on a clock that starts with the recording. The shot is the window from 1 s to 1 s + its length;
# the second either side is a handle. Prepare runs before the recording (opening the screen, finding what to tap and,
# where a list scrolls, a rehearsal of the same stroke); Act runs during it; Check confirms the result afterwards.
# Each take shows one action with a visible result, or two at most, and holds on the result.

function At([double]$Seconds) {
    $wait = [int](($Seconds - $script:clock.Elapsed.TotalSeconds) * 1000)
    if ($wait -gt 0) { Start-Sleep -Milliseconds $wait }
}

function Get-Center($Node) {
    $b = Get-Bounds $Node
    return @([int](($b[0] + $b[2]) / 2), [int](($b[1] + $b[3]) / 2))
}

function Tap-At([int[]]$Point) {
    $null = Invoke-Adb @('shell', 'input', 'tap', "$($Point[0])", "$($Point[1])")
}

function Warm-Up([int[]]$Point, [string]$Expect) {
    # The first time a dialog or screen opens the app loads its classes and inflates its views, which can cost a frame
    # or two on camera: open it once before recording, then go back.
    Tap-At $Point
    $null = Find-Ui $Expect
    $null = Invoke-Adb @('shell', 'input', 'keyevent', 'KEYCODE_BACK')
    Start-Sleep -Milliseconds 900
}

$plans = @{
    # The PC list, then a tap on Living-room PC opens its library.
    'hosts' = @{ Landscape = $false; FirstAction = 2.3
        Prepare = {
            Open-DemoState 'hosts'
            $script:target = Get-Center (Find-Ui 'Living-room PC')
            Warm-Up $script:target '10 games'
        }
        Act = { At 2.3; Tap-At $script:target }
        Check = { $null = Find-Ui 'Racing Game' }
    }
    # Living-room PC's settings: the Rubylight host as the subtitle, one fling to Display on the PC and Connection, then
    # the display-mode picker.
    'pc-settings' = @{ Landscape = $false; FirstAction = 2.3
        Prepare = {
            Open-DemoState 'pc-settings'
            Invoke-Fling 1950 650 300
            Start-Sleep -Milliseconds 1800
            $script:target = Get-Center (Find-Ui $script:labels['PC display mode'])
            if ($script:target[1] -lt 450 -or $script:target[1] -gt 1500) { throw "PC display mode stopped at y=$($script:target[1])." }
            Warm-Up $script:target 'Save display mode'
            Reset-ListToTop
        }
        Act = { At 2.3; Invoke-Fling 1950 650 300; At 4.7; Tap-At $script:target }
        Check = { $null = Find-Ui 'Save display mode' }
    }
    # The library in landscape, then a tap on Racing Game starts the stream with the compact overlay.
    'stream' = @{ Landscape = $true; FirstAction = 2.2
        Prepare = {
            Open-DemoState 'library'
            $script:target = Get-Center (Find-Ui 'Racing Game' -Scroll)
            # Start a stream once and leave it through the menu's Disconnect, so the take's start has warm code.
            Tap-At $script:target
            $dot = [string][char]0x00B7
            $null = Find-Ui "DEMO $dot local sample $dot scripted stats"
            Start-Sleep -Seconds 1
            Open-StreamMenu
            Tap-Ui 'Disconnect' -Scroll
            $null = Find-Ui 'Racing Game'
            Start-Sleep -Milliseconds 800
        }
        Act = { At 2.2; Tap-At $script:target }
        Check = { $dot = [string][char]0x00B7; $null = Find-Ui "DEMO $dot local sample $dot scripted stats" }
    }
    # Back in the stream opens the stream menu (quick toggles with the microphone); two slow strokes then scroll it to
    # Quality and Host.
    'menu' = @{ Landscape = $true; FirstAction = 2.0
        Prepare = {
            Open-DemoState 'compact'
            # The first opening inflates the sheet, which stalls the stream for a few frames: open and close it once.
            Open-StreamMenu
            $null = Invoke-Adb @('shell', 'input', 'keyevent', 'KEYCODE_BACK')
            Start-Sleep -Milliseconds 1200
        }
        Act = { At 2.0; $null = Invoke-Adb @('shell', 'input', 'keyevent', 'KEYCODE_BACK'); At 4.3; Show-StreamMenuSections }
        Check = { $null = Find-Ui 'Automatic stream quality' }
    }
    # A long press on the compact overlay switches it to Statistics, with the host's CPU and GPU readings.
    'statistics' = @{ Landscape = $true; FirstAction = 2.0
        Prepare = {
            Open-DemoState 'compact'
            $script:target = Get-Center (Find-Ui "${package}:id/performanceOverlay")
            # Once round the modes (Statistics, Debug, back to Compact), so the switch on camera lays out text it has
            # laid out before.
            foreach ($mode in 1..3) {
                $null = Invoke-Adb @('shell', 'input', 'swipe', "$($script:target[0])", "$($script:target[1])",
                    "$($script:target[0])", "$($script:target[1])", '700')
                Start-Sleep -Milliseconds 900
            }
        }
        Act = {
            At 2.0
            $null = Invoke-Adb @('shell', 'input', 'swipe', "$($script:target[0])", "$($script:target[1])",
                "$($script:target[0])", "$($script:target[1])", '700')
        }
        Check = {
            $overlay = (Get-Ui).SelectSingleNode("//node[@resource-id='${package}:id/performanceOverlay']")
            if (-not $overlay -or $overlay.GetAttribute('text') -notmatch 'Host CPU') { throw 'The overlay did not switch to Statistics.' }
        }
    }
    # Video codec lists PyroWave; back on the settings page, Display & image shows adaptive refresh on.
    'codec-refresh' = @{ Landscape = $false; FirstAction = 1.9
        Prepare = {
            # Not the 'settings' state, which applies the Native preset (adaptive refresh off) for the presets still.
            Open-DemoState 'settings-video'
            $null = Invoke-Adb @('shell', 'input', 'keyevent', 'KEYCODE_BACK')
            $script:section = Get-Center (Find-Ui $script:labels['Display & image'])
            Warm-Up $script:section 'Adaptive refresh (ARR / VRR)'
            Tap-Ui $script:labels['Video']
            Start-Sleep -Milliseconds 900
            $script:target = Get-Center (Find-Ui $script:labels['Video codec'])
            Warm-Up $script:target 'PyroWave (host support required)'
        }
        Act = {
            At 1.9; Tap-At $script:target
            At 4.0; $null = Invoke-Adb @('shell', 'input', 'keyevent', 'KEYCODE_BACK')
            At 4.45; $null = Invoke-Adb @('shell', 'input', 'keyevent', 'KEYCODE_BACK')
            At 4.95; Tap-At $script:section
        }
        Check = { $null = Find-Ui 'Adaptive refresh (ARR / VRR)' }
    }
    # Display & image: the upscaling choices, Sharp (FSR) and Sharp, Snapdragon (SGSR).
    'upscaling' = @{ Landscape = $false; FirstAction = 2.0
        Prepare = {
            Open-DemoState 'settings-display'
            # Client-side upscaling sits behind the section's More row (settings tiers).
            Tap-Ui $script:labels['More display'] -Scroll -Prefix
            Start-Sleep -Milliseconds 600
            $script:target = Get-Center (Find-Ui $script:labels['Client-side upscaling'] -Scroll)
            Warm-Up $script:target $script:labels['Sharp FSR']
        }
        Act = { At 2.0; Tap-At $script:target }
        Check = { $null = Find-Ui 'Sharp, Snapdragon (SGSR)' }
    }
    # Advanced: the Snapdragon GPU driver screen, as on an arm64 Adreno phone (a debug-only demo screen).
    'gpu-driver' = @{ Landscape = $false; FirstAction = 2.0
        Prepare = {
            Open-DemoState 'settings-advanced'
            $script:target = Get-Center (Find-Ui $script:labels['GPU driver'])
            Warm-Up $script:target 'Import driver ZIP'
        }
        Act = { At 2.0; Tap-At $script:target }
        Check = { $null = Find-Ui 'Import driver ZIP' }
    }
    # Controls: one fling to the USB controllers group and its DualSense rows.
    'usb' = @{ Landscape = $false; FirstAction = 2.0
        Prepare = {
            Open-DemoState 'settings-controls'
            Show-UsbControllers
            $null = Find-Ui 'USB DualSense audio and haptics'
            Reset-ListToTop
        }
        Act = { At 2.0; Invoke-Fling 2150 700 450 }
        Check = {
            $header = (Get-Bounds (Find-Ui $script:labels['USB controllers']))[1]
            if ($header -lt 380 -or $header -gt 900) { throw "USB controllers stopped at y=$header." }
            $null = Find-Ui 'USB DualSense audio and haptics'
        }
    }
    # Home sends the stream into Android's picture-in-picture window over the plain demo home screen.
    'pip' = @{ Landscape = $false; FirstAction = 2.2
        Prepare = { Open-DemoState 'pip' }
        Act = { At 2.2; $null = Invoke-Adb @('shell', 'input', 'keyevent', 'KEYCODE_HOME') }
        Check = {
            Start-Sleep -Seconds 1
            if ((Invoke-Adb @('shell', 'dumpsys', 'activity', 'activities')) -notmatch 'mode=pinned|windowingMode=2|mIsInPipMode=true') {
                throw 'PiP did not open.'
            }
        }
    }
}

# ---- 60 fps normalization -------------------------------------------------------------------------------------------
# screenrecord writes a frame only when the screen changes, stamped with the time it was composed. Each frame is put
# on the 60 fps grid at its time, shifted by the recorder's start-up delay so the clip's clock is the plan's clock.
# Frames composed a little early or late would land two on one slot and none on the next (a skipped and a repeated
# frame): a frame whose slot is taken moves to the next one, so every recorded frame is shown once. Slots between
# frames (a still screen) repeat the last one.

function Get-FrameTimes([string]$Path) {
    $times = & $ffprobe -v error -select_streams v:0 -show_entries packet=pts_time -of csv=p=0 $Path
    if ($LASTEXITCODE -ne 0) { throw "ffprobe failed: $Path" }
    return @($times | Where-Object { $_ -match '\d' } |
        ForEach-Object { [double]::Parse($_.Trim(',', ' '), [Globalization.CultureInfo]::InvariantCulture) } | Sort-Object)
}

function Read-Frame([IO.Stream]$Stream, [byte[]]$Buffer) {
    $read = 0
    while ($read -lt $Buffer.Length) {
        $count = $Stream.Read($Buffer, $read, $Buffer.Length - $read)
        if ($count -le 0) { return $false }
        $read += $count
    }
    return $true
}

function Get-Slots([double[]]$Times, [double]$Offset) {
    # Within half a slot (8 ms) the timing does not matter: use the shift that leaves the fewest frames sharing a slot.
    $best = [int]::MaxValue
    foreach ($phase in -0.45, -0.35, -0.25, -0.15, -0.05, 0.05, 0.15, 0.25, 0.35, 0.45) {
        $shared = 0
        for ($i = 1; $i -lt $Times.Count; $i++) {
            if ([Math]::Round(($Times[$i] - $Times[0] + $Offset) * $fps + $phase) -eq
                    [Math]::Round(($Times[$i - 1] - $Times[0] + $Offset) * $fps + $phase)) { $shared++ }
        }
        if ($shared -lt $best) { $best = $shared; $shift = $phase }
    }
    $slots = New-Object int[] $Times.Count
    $last = -1
    $moved = 0
    for ($i = 0; $i -lt $Times.Count; $i++) {
        $wanted = [int][Math]::Round(($Times[$i] - $Times[0] + $Offset) * $fps + $shift)
        # One slot late at most; a frame that would have to wait longer is dropped (screenrecord rarely gets that far ahead).
        $slots[$i] = if ($wanted -gt $last) { $wanted } elseif ($last + 1 -le $wanted + 1) { $last + 1 } else { -1 }
        if ($slots[$i] -ne $wanted) { $moved++ }
        if ($slots[$i] -ge 0) { $last = $slots[$i] }
    }
    # Refreshes the app or the recorder missed while something moved: runs of one to eight empty slots between frames
    # (longer runs are a still screen), and frames dropped above.
    $missed = 0
    $previous = -1
    foreach ($slot in $slots) {
        if ($slot -lt 0) { $missed++; continue }
        if ($previous -ge 0 -and $slot - $previous -ge 2 -and $slot - $previous -le 9) { $missed += $slot - $previous - 1 }
        $previous = $slot
    }
    return [pscustomobject]@{ Slots = $slots; Moved = $moved; Missed = $missed }
}

function ConvertTo-Cfr([string]$Source, [string]$Target, [double]$Offset, [double]$Seconds, [int]$Width, [int]$Height) {
    $times = Get-FrameTimes $Source
    $plan = Get-Slots $times $Offset
    $slots = $plan.Slots
    $count = [int][Math]::Round($Seconds * $fps)
    $frameBytes = $Width * $Height * 3 / 2
    $decoder = New-Object Diagnostics.ProcessStartInfo $ffmpeg
    # Microsecond output timestamps: rawvideo would otherwise complain about frames that share a coarser tick.
    $decoder.Arguments = "-hide_banner -nostdin -loglevel error -i `"$Source`" -fps_mode passthrough -enc_time_base 1/1000000 " +
        "-f rawvideo -pix_fmt yuv420p -"
    $decoder.RedirectStandardOutput = $true
    $decoder.UseShellExecute = $false
    $encoder = New-Object Diagnostics.ProcessStartInfo $ffmpeg
    $encoder.Arguments = "-hide_banner -nostdin -y -loglevel error -f rawvideo -pix_fmt yuv420p -s ${Width}x$Height -framerate $fps -i - " +
        "-an -c:v libx264 -preset fast -crf 12 -pix_fmt yuv420p -movflags +faststart `"$Target`""
    $encoder.RedirectStandardInput = $true
    $encoder.UseShellExecute = $false
    $in = [Diagnostics.Process]::Start($decoder)
    $out = [Diagnostics.Process]::Start($encoder)
    try {
        $reader = $in.StandardOutput.BaseStream
        $writer = $out.StandardInput.BaseStream
        $frame = New-Object byte[] $frameBytes
        $next = New-Object byte[] $frameBytes
        if (-not (Read-Frame $reader $frame)) { throw "No frames in $Source" }
        $index = 0
        $hasNext = Read-Frame $reader $next
        for ($slot = 0; $slot -lt $count; $slot++) {
            # Move on to the last recorded frame whose slot has come; dropped frames (slot -1) are passed over.
            while ($hasNext -and $index + 1 -lt $slots.Count -and $slots[$index + 1] -le $slot) {
                $index++
                if ($slots[$index] -ge 0) { $swap = $frame; $frame = $next; $next = $swap }
                $hasNext = Read-Frame $reader $next
            }
            $writer.Write($frame, 0, $frame.Length)
        }
        $writer.Close()
        $out.WaitForExit()
        if ($out.ExitCode -ne 0) { throw "FFmpeg could not encode $Target." }
    } finally {
        if (-not $in.HasExited) { $in.Kill() }
        if (-not $out.HasExited) { $out.Kill() }
    }
    return $plan.Moved
}

# ---- Capture --------------------------------------------------------------------------------------------------------

function Wait-QuietHost {
    $deadline = (Get-Date).AddMinutes(5)
    $quiet = 0
    while ($quiet -lt 3) {
        $load = (Get-CimInstance Win32_Processor | Measure-Object LoadPercentage -Average).Average
        $quiet = $(if ($load -lt $MaxHostLoad) { $quiet + 1 } else { 0 })
        if ((Get-Date) -gt $deadline) { Write-Warning "The host is still busy ($load% CPU); recording anyway."; return }
        if ($quiet -lt 3) { Start-Sleep -Seconds 2 }
    }
}

function Invoke-Take($Take, $Plan, [int]$Width, [int]$Height, [double]$Seconds, [string]$Raw) {
    Set-Orientation $Plan.Landscape
    & $Plan.Prepare
    if ($Plan.Landscape) { Start-Sleep -Seconds 1 }
    Start-Sleep -Milliseconds 1200
    if ($MaxHostLoad -lt 100) { Wait-QuietHost }
    $remoteClip = "$remote/$($Take.Id).mp4"
    if ($HostCapture) {
        # The window's display area, grabbed at 60 fps. The emulator draws a landscape app turned on its side inside
        # the portrait window, so landscape takes are turned back (counter-clockwise).
        $r = $script:window
        $grab = "ddagrab=output_idx=$OutputIndex`:framerate=$fps`:offset_x=$($r.X):offset_y=$($r.Y):video_size=$($r.Width)x$($r.Height)"
        $turn = $(if ($Plan.Landscape) { ',transpose=2' } else { '' })
        $recording = Start-Process -FilePath $ffmpeg -ArgumentList @('-hide_banner', '-nostdin', '-y', '-loglevel', 'error',
            '-f', 'lavfi', '-i', $grab, '-vf', "hwdownload,format=bgra$turn,format=nv12", '-t', "$($Seconds + 1)",
            '-c:v', 'h264_amf', '-quality', 'quality', '-rc', 'cqp', '-qp_i', '16', '-qp_p', '18', $Raw) `
            -WindowStyle Hidden -PassThru -RedirectStandardError (Join-Path $output "$($Take.Id).record-error.log")
    } else {
        $recording = Start-Process -FilePath $adb -ArgumentList @('shell', 'screenrecord', '--size', "${Width}x$Height",
            '--bit-rate', '12000000', '--time-limit', "$($Seconds + 2)", $remoteClip) `
            -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $output "$($Take.Id).record.log") `
            -RedirectStandardError (Join-Path $output "$($Take.Id).record-error.log")
    }
    $script:clock = [Diagnostics.Stopwatch]::StartNew()
    # PowerShell 7 leaves ExitCode empty for a -PassThru process unless its handle was opened before it exited.
    $null = $recording.Handle
    try {
        & $Plan.Act
        if (-not $recording.WaitForExit(($Seconds + 20) * 1000)) { throw 'screenrecord timed out.' }
        if ($recording.ExitCode -ne 0) { throw "The recording failed: $($Take.Id).record-error.log" }
    } finally {
        if (-not $recording.HasExited) { $recording.Kill() }
        $recording.Dispose()
    }
    & $Plan.Check
    if (-not $HostCapture) {
        $null = Invoke-Adb @('pull', $remoteClip, $Raw)
        $null = Invoke-Adb @('shell', 'rm', $remoteClip)
    }
    if ((Get-Item $Raw).Length -eq 0) { throw 'Empty recording.' }
}

function Get-Repeats([string]$Path) {
    # A grab has a frame every 1/60 s whether or not the window changed: count the repeated ones FFmpeg's mpdecimate
    # finds while something moves (runs of one to eight repeats; longer runs are a still screen).
    $log = Join-Path $output 'repeats.log'
    $ErrorActionPreference = 'Continue'
    & $ffmpeg -hide_banner -nostdin -i $Path -vf 'mpdecimate=hi=64*4:lo=64*2:frac=0.2,showinfo' -an -f null - 2> $log
    $ErrorActionPreference = 'Stop'
    $kept = @(Select-String -LiteralPath $log -Pattern 'pts_time:([0-9.]+)' | ForEach-Object {
        [double]::Parse($_.Matches[0].Groups[1].Value, [Globalization.CultureInfo]::InvariantCulture) })
    $repeats = 0
    for ($i = 1; $i -lt $kept.Count; $i++) {
        $gap = [int][Math]::Round(($kept[$i] - $kept[$i - 1]) * $fps) - 1
        if ($gap -ge 1 -and $gap -le 8) { $repeats += $gap }
    }
    return $repeats
}

# The recorder's start-up delay where a take cannot measure it (a stream never stands still before its first action).
$startDelay = $(if ($HostCapture) { 0.35 } else { 0.45 })
if ($HostCapture) {
    $OutputIndex = 0
    $script:window = & (Join-Path $PSScriptRoot 'emulator-window.ps1') -Height $WindowHeight
    if ($script:window.Width % 2 -or $script:window.Height % 2) { throw "Make the window's display area even-sized: $($script:window.Width)x$($script:window.Height)" }
    $portraitSize = @($script:window.Width, $script:window.Height)
    Write-Host "Recording the emulator window's display area at $($script:window.X),$($script:window.Y), $($script:window.Width)x$($script:window.Height)"
}
try {
    Initialize-Capture $Theme
    # A plain dark home screen for the picture-in-picture shot (enabling it launches an activity, so do it before any stream).
    if ($shots | Where-Object State -eq 'pip') { Enable-DemoHome }
    foreach ($take in $shots) {
        $plan = $plans[$take.State]
        if (-not $plan) { throw "No capture plan for $($take.Id) ($($take.State))." }
        $width = $portraitSize[$(if ($plan.Landscape) { 1 } else { 0 })]
        $height = $portraitSize[$(if ($plan.Landscape) { 0 } else { 1 })]
        $seconds = $take.Seconds + 2
        # The recording as it came is kept beside the clip (out/ is not committed) for checking the timing, with the
        # number of refreshes it missed while something moved.
        $raw = Join-Path $output "$($take.Id).raw.mp4"
        $score = Join-Path $output "$($take.Id).missed.txt"
        $candidate = Join-Path $output "$($take.Id).take.mp4"
        $kept = $(if (-not $Replace -and (Test-Path -LiteralPath $raw) -and (Test-Path -LiteralPath $score)) {
            [int](Get-Content -LiteralPath $score) } else { $null })
        # An emulator app now and then misses a frame or two in a transition: take the shot up to -Takes times and keep
        # the take with the fewest missed frames, which may still be the clip from an earlier run.
        $best = $null
        $failures = 0
        for ($attempt = 1; $attempt -le $Takes -and ($null -eq $best -or $best -gt 0); ) {
            try {
                Invoke-Take $take $plan $width $height $seconds $candidate
                $hitches = $(if ($HostCapture) { Get-Repeats $candidate } else { (Get-Slots (Get-FrameTimes $candidate) 0).Missed })
                Write-Host "  $($take.Id) take $attempt`: $hitches missed frame(s) in motion"
                if (($null -eq $best -or $hitches -lt $best) -and ($null -eq $kept -or $hitches -lt $kept)) {
                    Move-Item -Force $candidate $raw
                    Set-Content -LiteralPath $score $hitches
                    $best = $hitches
                }
                $attempt++
            } catch {
                if (++$failures -ge 3) { throw }
                Write-Warning "Retaking $($take.Id): $($_.Exception.Message)"
            }
        }
        if (Test-Path -LiteralPath $candidate) { Remove-Item -LiteralPath $candidate }
        if ($null -eq $best) {
            Write-Host "Kept $($take.Id).mp4 from an earlier run: $kept missed in motion"
            continue
        }
        # A screen that stood still until the first action gives the recorder's start-up delay: its second frame is the
        # action's first visible change. The clip is shifted so that change lands on the planned time.
        $times = Get-FrameTimes $raw
        $delay = $startDelay
        if ($times.Count -gt 1) {
            $measured = $plan.FirstAction - ($times[1] - $times[0])
            if ($measured -ge 0 -and $measured -le 1.5) { $delay = $measured }
        }
        $moved = ConvertTo-Cfr $raw (Join-Path $output "$($take.Id).mp4") $delay $seconds $width $height
        Write-Host ('Saved {0}.mp4: {1} frames recorded, {2} missed in motion, {3} moved off a taken slot, start-up delay {4:0.00} s' -f `
            $take.Id, $times.Count, $best, $moved, $delay)
    }
} finally { Restore-Capture }
