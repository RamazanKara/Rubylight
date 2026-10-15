# -Theme picks the system theme the app follows (dark by default, matching the stills); the prior mode is restored.
param([string[]]$Shot, [ValidateSet('dark', 'light')][string]$Theme = 'dark')
. (Join-Path $PSScriptRoot 'capture-common.ps1')
$output = Join-Path $root 'out/demo/clips'
New-Item -ItemType Directory -Force $output | Out-Null
$shots = @(Get-Content (Join-Path $root 'docs/demo/shotlist.md') | ForEach-Object {
    if ($_ -match '^\| \d{2}-') {
        $cells = $_.Split('|').Trim()
        [pscustomobject]@{ Id = $cells[1]; Seconds = [int]$cells[2]; State = $cells[8] }
    }
})
# Stream shots are recorded in landscape so the sample fills the screen (in portrait it is a thin strip between black bars).
$ffmpeg = Find-FFmpeg
$ffprobe = Join-Path (Split-Path -Parent $ffmpeg) 'ffprobe.exe'
$landscapeShots = @('05-stream', '06-compact', '07-advanced', '11-touch', '13-performance')
if ($Shot) {
    foreach ($id in $Shot) {
        if ($id -notin $shots.Id) { throw "Unknown shot: $id" }
    }
    $shots = @($shots | Where-Object { $_.Id -in $Shot })
}
try {
    Initialize-Capture $Theme
    # A plain dark home screen for the picture-in-picture shot (enabling it launches an activity, so do it before any stream).
    if ($shots | Where-Object State -eq 'pip') { Enable-DemoHome }
    foreach ($take in $shots) {
        $node = $null
        $landscape = $take.Id -in $landscapeShots
        Set-Orientation $landscape
        # Half resolution: the emulator's software H.264 encoder cannot keep up at 1080x2400, and the render never shows the phone larger than this.
        $size = if ($landscape) { '1200x540' } else { '540x1200' }
        if ($take.Id -eq '04-library') {
            # Taps on the PC cards do not navigate on the emulator (the grid's item click never fires), so show the library directly.
            Open-DemoState 'library'
        } elseif ($take.Id -eq '05-stream') {
            Open-DemoState 'library'
            $node = Find-Ui 'Racing Game' -Scroll
        } else { Open-DemoState $(if ($take.State -eq 'toggle') { 'compact' } else { $take.State }) }
        if ($landscape) { Start-Sleep -Seconds 2 }
        $seconds = $take.Seconds + 2
        $remoteClip = "$remote/$($take.Id).mp4"
        $recording = Start-Process -FilePath $adb -ArgumentList @('shell', 'screenrecord',
            '--size', $size, '--bit-rate', '8000000', '--time-limit', "$seconds", $remoteClip) `
            -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $output "$($take.Id).record.log") `
            -RedirectStandardError (Join-Path $output "$($take.Id).record-error.log")
        # PowerShell 7 leaves ExitCode empty for a -PassThru process unless its handle was opened before it exited.
        $null = $recording.Handle
        try {
            Start-Sleep -Seconds 1
            if ($node) { Tap-Node $node; Hide-Ime -WaitForShow }
            if ($take.State -eq 'pip') { Enter-DemoPip }
            if ($take.State -eq 'toggle') {
                # Show both overlay modes: after the compact pill has been on screen for a while, long-press it, which is how the app
                # switches to the advanced list (the pill is top centre in landscape).
                Start-Sleep -Milliseconds 2700
                $null = Invoke-Adb @('shell', 'input', 'swipe', '1200', '60', '1200', '60', '900')
            }
            if (-not $recording.WaitForExit(($seconds + 15) * 1000)) { throw 'screenrecord timed out.' }
            if ($recording.ExitCode -ne 0) { throw "screenrecord failed: $($take.Id).record-error.log" }
            $temporary = Join-Path $output "$($take.Id).capture.mp4"
            $null = Invoke-Adb @('pull', $remoteClip, $temporary)
            if ((Get-Item $temporary).Length -eq 0) { throw 'Empty recording.' }
            # screenrecord writes variable-rate video and stops at the last changed frame, and a static screen yields
            # a single frame: make every clip constant 30 fps and hold the last frame for the full length.
            $final = Join-Path $output "$($take.Id).mp4"
            $packets = & $ffprobe -v error -count_packets -select_streams v:0 -show_entries stream=nb_read_packets -of csv=p=0 $temporary
            $encode = @('-an', '-c:v', 'libx264', '-preset', 'fast', '-crf', '14', '-pix_fmt', 'yuv420p', '-movflags', '+faststart')
            if ([int]$packets -le 1) {
                $frame = Join-Path $output "$($take.Id).frame.png"
                $null = & $ffmpeg -hide_banner -nostdin -y -loglevel error -i $temporary -frames:v 1 -update 1 $frame 2>&1
                $null = & $ffmpeg -hide_banner -nostdin -y -loglevel error -loop 1 -framerate 30 -i $frame -t $seconds @encode $final 2>&1
                Remove-Item -LiteralPath $frame
            } else {
                $null = & $ffmpeg -hide_banner -nostdin -y -loglevel error -i $temporary `
                    -vf 'fps=30:round=near,tpad=stop_mode=clone:stop_duration=10' -t $seconds @encode $final 2>&1
            }
            if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $final) -or (Get-Item -LiteralPath $final).Length -eq 0) {
                throw "FFmpeg could not normalize $($take.Id)."
            }
            Remove-Item -LiteralPath $temporary
            $null = Invoke-Adb @('shell', 'rm', $remoteClip)
            Write-Host "Saved $($take.Id).mp4"
        } finally {
            if (-not $recording.HasExited) { $recording.Kill() }
            $recording.Dispose()
        }
    }
} finally { Restore-Capture }
