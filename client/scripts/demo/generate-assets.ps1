$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$assets = Join-Path $root 'app/src/debug/assets/demo'
$work = Join-Path $root 'out/demo/assets'
New-Item -ItemType Directory -Force $assets, $work | Out-Null
$command = Get-Command ffmpeg.exe -ErrorAction SilentlyContinue
$ffmpeg = if ($command) { $command.Source } else {
    Get-ChildItem "$env:LOCALAPPDATA/Microsoft/WinGet/Packages/Gyan.FFmpeg*/**/bin/ffmpeg.exe" -ErrorAction SilentlyContinue |
        Select-Object -First 1 -ExpandProperty FullName
}
if (-not $ffmpeg) { throw 'Put the installed FFmpeg bin directory on PATH.' }
$java = Join-Path $env:JAVA_HOME 'bin/java.exe'
& $java (Join-Path $PSScriptRoot 'GenerateAssets.java') $assets $ffmpeg (Join-Path $work 'sample.png')
if ($LASTEXITCODE -ne 0) { throw 'Asset generation failed.' }
$encoders = (& $ffmpeg -hide_banner -encoders 2>&1) -join "`n"
if ($encoders -match 'libx265') {
    & $ffmpeg -y -hide_banner -loglevel warning -i "$assets/stream-h264.mp4" -an -c:v libx265 -preset fast -crf 23 `
        -pix_fmt yuv420p10le -tag:v hvc1 -x265-params 'pools=2:frame-threads=2:log-level=error' -movflags +faststart "$assets/stream-hevc.mp4"
    if ($LASTEXITCODE -ne 0) { throw 'HEVC encode failed.' }
}
# libaom first: SVT-AV1 at these settings writes a sample about 3.5 times larger (6 MB instead of 1.7 MB) into the debug APK.
if ($encoders -match 'libaom-av1') {
    & $ffmpeg -y -hide_banner -loglevel warning -i "$assets/stream-h264.mp4" -an -c:v libaom-av1 -cpu-used 8 -crf 36 -b:v 0 `
        -row-mt 1 -threads 2 -pix_fmt yuv420p10le -movflags +faststart "$assets/stream-av1.mp4"
    if ($LASTEXITCODE -ne 0) { throw 'AV1 encode failed.' }
} elseif ($encoders -match 'libsvtav1') {
    & $ffmpeg -y -hide_banner -loglevel warning -i "$assets/stream-h264.mp4" -an -c:v libsvtav1 -preset 10 -crf 32 `
        -pix_fmt yuv420p10le -svtav1-params 'lp=2' -movflags +faststart "$assets/stream-av1.mp4"
    if ($LASTEXITCODE -ne 0) { throw 'AV1 encode failed.' }
}
$ffprobe = Join-Path (Split-Path $ffmpeg) 'ffprobe.exe'
Get-ChildItem "$assets/stream-*.mp4" | ForEach-Object {
    $json = & $ffprobe -v error -select_streams v:0 -show_entries stream=codec_name,width,height,r_frame_rate,pix_fmt,nb_frames:format=duration -of json $_.FullName
    if ($LASTEXITCODE -ne 0) { throw "Probe failed: $_" }
    $json | Set-Content (Join-Path $work "$($_.BaseName).json")
    $probe = ($json -join "`n") | ConvertFrom-Json
    if ($probe.streams[0].width -ne 1920 -or $probe.streams[0].height -ne 1080 -or
        $probe.streams[0].r_frame_rate -ne '60/1' -or [double]$probe.format.duration -ne 10) {
        throw "Unexpected sample format: $_"
    }
}
