param([switch]$Storyboard)

$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$output = Join-Path $root 'out/demo'
$work = Join-Path $output 'work'
$frames = Join-Path $output 'frames'
$utf8 = New-Object Text.UTF8Encoding($false)
$culture = [Globalization.CultureInfo]::InvariantCulture
$fps = 30
$fade = 0.4
$intro = 2.5
$outro = 4.5
$releases = 'https://github.com/RamazanKara/rubylight-android/releases'

function Find-FFmpeg {
    $command = Get-Command ffmpeg.exe -ErrorAction SilentlyContinue
    if ($command) { return $command.Source }
    $winget = Join-Path $env:LOCALAPPDATA 'Microsoft/WinGet'
    $link = Join-Path $winget 'Links/ffmpeg.exe'
    if (Test-Path -LiteralPath $link) {
        $target = (Get-Item -LiteralPath $link).Target
        if ($target -and (Test-Path -LiteralPath $target -ErrorAction SilentlyContinue)) { return $target }
        if (-not $target) { return $link }
    }
    $binary = Get-ChildItem -Path (Join-Path $winget 'Packages/Gyan.FFmpeg_*/*/bin/ffmpeg.exe') -ErrorAction SilentlyContinue |
        Sort-Object FullName | Select-Object -Last 1
    if ($binary) { return $binary.FullName }
    throw 'FFmpeg is not accessible. Put a readable installed FFmpeg bin directory on PATH.'
}

function Number([double]$Value) { return $Value.ToString('0.######', $culture) }

function Invoke-FFmpeg([string[]]$Arguments, [string]$Log) {
    # Windows PowerShell can promote successful native-tool diagnostics to errors.
    $ErrorActionPreference = 'Continue'
    & $ffmpeg -hide_banner -nostdin -y -loglevel info @Arguments 2> (Join-Path $work "$Log.log")
    $ErrorActionPreference = 'Stop'
    if ($LASTEXITCODE -ne 0) {
        $tail = Get-Content -LiteralPath (Join-Path $work "$Log.log") -Tail 18
        throw "FFmpeg failed ($Log):`n$($tail -join "`n")"
    }
}

function Get-Media([string]$Path) {
    $json = & $ffprobe -v error -show_streams -show_format -of json $Path
    if ($LASTEXITCODE -ne 0) { throw "ffprobe failed: $Path" }
    return ($json -join "`n" | ConvertFrom-Json)
}

function Text-Filter([string]$Name, [string]$Text, [int]$Size, [string]$X, [int]$Y,
        [string]$Color = 'F0F3F4', [string]$Font = 'bold') {
    [IO.File]::WriteAllText((Join-Path $work "$Name.txt"), $Text, $utf8)
    return "drawtext=fontfile=font-$Font.ttf:textfile=$Name.txt:expansion=none:fontsize=${Size}:fontcolor=0x${Color}:x=${X}:y=$Y"
}

function Text-Lines([string]$Name, [string[]]$Lines, [int]$Size, [string]$X, [int]$Y,
        [string]$Color = 'F0F3F4', [string]$Font = 'bold') {
    $filters = @()
    for ($line = 0; $line -lt $Lines.Count; $line++) {
        $filters += Text-Filter "$Name-$line" $Lines[$line] $Size $X ($Y + $line * ($Size + 16)) $Color $Font
    }
    return ($filters -join ',')
}

function Wrap-Caption([string]$Text) {
    $line = ''
    $lines = @()
    foreach ($word in $Text.Split(' ')) {
        if ($line -and ($line.Length + 1 + $word.Length) -gt 33) {
            $lines += $line
            $line = $word
        } elseif ($line) { $line += " $word" } else { $line = $word }
    }
    if ($line) { $lines += $line }
    return $lines
}

function Round-Rectangle([Drawing.Graphics]$Graphics, [Drawing.Brush]$Brush,
        [int]$X, [int]$Y, [int]$Width, [int]$Height, [int]$Radius) {
    $path = New-Object Drawing.Drawing2D.GraphicsPath
    try {
        $d = $Radius * 2
        $path.AddArc($X, $Y, $d, $d, 180, 90)
        $path.AddArc($X + $Width - $d, $Y, $d, $d, 270, 90)
        $path.AddArc($X + $Width - $d, $Y + $Height - $d, $d, $d, 0, 90)
        $path.AddArc($X, $Y + $Height - $d, $d, $d, 90, 90)
        $path.CloseFigure()
        $Graphics.FillPath($Brush, $path)
    } finally { $path.Dispose() }
}

function New-Backdrop([string]$Name, [int]$Width, [int]$Height, $Phone) {
    $bitmap = New-Object Drawing.Bitmap($Width, $Height)
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    $rect = New-Object Drawing.Rectangle(0, 0, $Width, $Height)
    $startColor = '#32101C'
    if (-not $Phone) { $startColor = '#C41242' }
    $gradient = New-Object Drawing.Drawing2D.LinearGradientBrush($rect,
        [Drawing.ColorTranslator]::FromHtml($startColor), [Drawing.ColorTranslator]::FromHtml('#0E0D13'), 35.0)
    $ruby = New-Object Drawing.SolidBrush([Drawing.ColorTranslator]::FromHtml('#C41242'))
    $rim = New-Object Drawing.SolidBrush([Drawing.ColorTranslator]::FromHtml('#536168'))
    $black = New-Object Drawing.SolidBrush([Drawing.ColorTranslator]::FromHtml('#080B0D'))
    $shadow = New-Object Drawing.SolidBrush([Drawing.Color]::FromArgb(70, 0, 0, 0))
    try {
        $graphics.SmoothingMode = [Drawing.Drawing2D.SmoothingMode]::AntiAlias
        $graphics.FillRectangle($gradient, $rect)
        $graphics.FillRectangle($ruby, 108, 204, 68, 4)
        if ($Phone) {
            $x = $Phone.X; $y = $Phone.Y; $w = $Phone.Width; $h = $Phone.Height
            Round-Rectangle $graphics $shadow ($x - 28) ($y - 8) ($w + 56) ($h + 48) 38
            Round-Rectangle $graphics $rim ($x - 18) ($y - 18) ($w + 36) ($h + 36) 28
            Round-Rectangle $graphics $black ($x - 16) ($y - 16) ($w + 32) ($h + 32) 26
            $graphics.FillRectangle($rim, ($x + $w + 18), ($y + 78), 4, 60)
        }
        $bitmap.Save((Join-Path $work $Name), [Drawing.Imaging.ImageFormat]::Png)
    } finally {
        $shadow.Dispose(); $black.Dispose(); $rim.Dispose(); $ruby.Dispose()
        $gradient.Dispose(); $graphics.Dispose(); $bitmap.Dispose()
    }
}

$shots = @(Get-Content -LiteralPath (Join-Path $root 'docs/demo/shotlist.md') | ForEach-Object {
    if ($_ -match '^\| \d{2}-') {
        $cells = $_.Split('|').Trim()
        [pscustomobject]@{
            Id = $cells[1]; Seconds = [int]$cells[2]; Caption = $cells[3]
            Heading = $cells[4]; Callout = $cells[5]; Still = $cells[6]; Crop = $cells[7]
        }
    }
})
if ($shots.Count -ne 13) { throw 'Expected 13 shots in docs/demo/shotlist.md.' }
$duration = $intro + $outro + ($shots | Measure-Object Seconds -Sum).Sum - ($shots.Count + 1) * $fade
$null = New-Item -ItemType Directory -Path $output, $work, $frames -Force
$ffmpeg = Find-FFmpeg
$ffprobe = Join-Path (Split-Path -Parent $ffmpeg) 'ffprobe.exe'
if (-not (Test-Path -LiteralPath $ffprobe)) { throw 'ffprobe.exe must be beside ffmpeg.exe.' }
$null = & $ffmpeg -hide_banner -version
if ($LASTEXITCODE -ne 0) { throw 'The installed FFmpeg could not run.' }
foreach ($font in @(@('bold', 'segoeuib.ttf'), @('regular', 'segoeui.ttf'))) {
    $path = Join-Path $env:WINDIR "Fonts/$($font[1])"
    if (-not (Test-Path -LiteralPath $path)) { throw "Required Windows font is missing: $path" }
    Copy-Item -LiteralPath $path -Destination (Join-Path $work "font-$($font[0]).ttf") -Force
}
Add-Type -AssemblyName System.Drawing
$sources = @()
foreach ($shot in $shots) {
    $clip = Join-Path $output "clips/$($shot.Id).mp4"
    $still = $Storyboard -or -not (Test-Path -LiteralPath $clip)
    $path = $clip
    if ($still) { $path = Join-Path $root "docs/screenshots/$($shot.Still)" }
    if (-not (Test-Path -LiteralPath $path)) { throw "Missing shot source: $path" }
    $media = Get-Media $path
    $video = @($media.streams | Where-Object codec_type -eq 'video')[0]
    if (-not $video) { throw "No video or image stream: $path" }
    if (-not $still) {
        if ($video.width -ne 1080 -or $video.height -ne 2400) { throw "Clip must be 1080x2400: $path" }
        if ([double]::Parse($media.format.duration, $culture) -lt ($shot.Seconds + 1)) {
            throw "Clip is too short for its one-second lead handle: $path"
        }
    }
    $sources += [pscustomobject]@{
        Id = $shot.Id; Kind = $(if ($still) { 'storyboard' } else { 'clip' })
        Path = $path; Width = $video.width; Height = $video.height; Seconds = $shot.Seconds
    }
}
[IO.File]::WriteAllText((Join-Path $output 'sources.json'), ($sources | ConvertTo-Json), $utf8)

# Relative filter paths avoid Windows drive-colon and apostrophe escaping in drawtext.
Push-Location $work
try {
    $encode = @('-an', '-c:v', 'libx264', '-preset', 'medium', '-crf', '18', '-pix_fmt', 'yuv420p',
        '-r', "$fps", '-threads', '2', '-video_track_timescale', '30000', '-movflags', '+faststart')
    foreach ($format in @('16x9', '9x16')) {
        $vertical = $format -eq '9x16'
        $width = 1920; $height = 1080
        if ($vertical) { $width = 1080; $height = 1920 }
        Write-Host "Rendering $format ($duration seconds)"
        New-Backdrop "$format-title.png" $width $height $null
        $segments = @()
        foreach ($card in @('intro', 'outro')) {
            $id = "$format-$card"
            $seconds = $intro
            $filters = @()
            if ($card -eq 'intro') {
                if ($vertical) {
                    $filters += Text-Filter "$id-brand" 'RUBYLIGHT' 112 '(w-text_w)/2' 720 'FFFFFF'
                    $filters += Text-Lines "$id-tagline" @('Your games. Your screen.', 'Your stream.') 44 '(w-text_w)/2' 925
                } else {
                    $filters += Text-Filter "$id-brand" 'RUBYLIGHT' 156 '(w-text_w)/2' 386 'FFFFFF'
                    $filters += Text-Filter "$id-tagline" 'Your games. Your screen. Your stream.' 46 '(w-text_w)/2' 610
                }
            } else {
                $seconds = $outro
                if ($vertical) {
                    $filters += Text-Filter "$id-brand" 'RUBYLIGHT' 80 '(w-text_w)/2' 465 'FFFFFF'
                    $filters += Text-Filter "$id-cta" 'Start your stream.' 76 '(w-text_w)/2' 740
                    $filters += Text-Filter "$id-get" 'Get RUBYLIGHT' 44 '(w-text_w)/2' 908 'FFFFFF'
                    $filters += Text-Lines "$id-url" @('https://github.com/RamazanKara', '/rubylight-android/releases') 34 '(w-text_w)/2' 1070 'BCC8CE' 'regular'
                } else {
                    $filters += Text-Filter "$id-brand" 'RUBYLIGHT' 72 '(w-text_w)/2' 230 'FFFFFF'
                    $filters += Text-Filter "$id-cta" 'Start your stream.' 98 '(w-text_w)/2' 425
                    $filters += Text-Filter "$id-get" 'Get RUBYLIGHT' 40 '(w-text_w)/2' 600 'FFFFFF'
                    $filters += Text-Filter "$id-url" $releases 34 '(w-text_w)/2' 710 'BCC8CE' 'regular'
                }
            }
            $filters += 'setsar=1,format=yuv420p'
            Invoke-FFmpeg (@('-loop', '1', '-framerate', "$fps", '-i', "$format-title.png", '-vf', ($filters -join ','),
                '-t', (Number $seconds)) + $encode + @("$id.mp4")) $id
        }
        $segments += [pscustomobject]@{ Path = "$format-intro.mp4"; Seconds = $intro }
        for ($index = 0; $index -lt $shots.Count; $index++) {
            $shot = $shots[$index]; $source = $sources[$index]
            $id = "$format-$($shot.Id)"
            $landscape = $source.Width -gt $source.Height
            $maxW = 910; $maxH = 810; $centerY = 525
            if ($vertical) { $maxW = 844; $maxH = 1180; $centerY = 920 }
            $scale = [Math]::Min($maxW / $source.Width, $maxH / $source.Height)
            $screenW = [int]([Math]::Floor($source.Width * $scale / 2) * 2)
            $screenH = [int]([Math]::Floor($source.Height * $scale / 2) * 2)
            $phone = [pscustomobject]@{
                X = [int](($width - $screenW) / 2); Y = [int]($centerY - $screenH / 2)
                Width = $screenW; Height = $screenH
            }
            New-Backdrop "$id-bg.png" $width $height $phone
            $inputs = @('-loop', '1', '-framerate', "$fps", '-i', "$id-bg.png")
            if ($source.Kind -eq 'storyboard') {
                $inputs += @('-i', $source.Path)
                $crop = ''
                if ($shot.Crop -ne 'full') { $crop = "crop=$($shot.Crop)," }
                $padW = [int]([Math]::Ceiling($screenW * 2.08 / 2) * 2)
                $padH = [int]([Math]::Ceiling($screenH * 2.08 / 2) * 2)
                $count = $shot.Seconds * $fps
                $zoom = "1+0.03*on/($count-1)"
                if ($index % 2 -eq 1) { $zoom = "1.03-0.03*on/($count-1)" }
                $screen = "[1:v]${crop}scale=$($screenW * 2):$($screenH * 2):force_original_aspect_ratio=decrease:flags=lanczos," +
                    "pad=${padW}:${padH}:(ow-iw)/2:(oh-ih)/2:color=0x111A1F," +
                    "zoompan=z='$zoom':x='iw/2-iw/zoom/2':y='ih/2-ih/zoom/2':d=${count}:s=${screenW}x${screenH}:fps=${fps},setsar=1[screen]"
            } else {
                $inputs += @('-ss', '1', '-i', $source.Path)
                $screen = "[1:v]trim=duration=$($shot.Seconds),setpts=PTS-STARTPTS,fps=${fps}," +
                    "scale=${screenW}:${screenH}:flags=lanczos,setsar=1[screen]"
            }
            $filters = @()
            if ($vertical) {
                $filters += Text-Filter "$id-brand" 'RUBYLIGHT' 42 '108' 130 'FFFFFF'
                $filters += Text-Filter "$id-count" ('{0:00} / 13' -f ($index + 1)) 26 '824' 143 'BCC8CE' 'regular'
                $filters += Text-Filter "$id-heading" $shot.Heading.Replace('~', ' ') 52 '(w-text_w)/2' 240
                $filters += Text-Lines "$id-caption" @(Wrap-Caption $shot.Caption) 44 '(w-text_w)/2' 1610
                if ($landscape) {
                    $filters += Text-Lines "$id-callout" $shot.Callout.Split('~') 34 '(w-text_w)/2' 1320 'BCC8CE' 'regular'
                }
            } else {
                $filters += Text-Filter "$id-brand" 'RUBYLIGHT' 36 '108' 92 'FFFFFF'
                $filters += Text-Filter "$id-count" ('{0:00} / 13' -f ($index + 1)) 26 '1686' 99 'BCC8CE' 'regular'
                $filters += Text-Lines "$id-heading" $shot.Heading.Split('~') 56 '108' 404
                $filters += Text-Lines "$id-callout" $shot.Callout.Split('~') 27 '1450' 446 'BCC8CE' 'regular'
                $filters += Text-Filter "$id-caption" $shot.Caption 42 '(w-text_w)/2' 960
            }
            $graph = "$screen;[0:v][screen]overlay=$($phone.X):$($phone.Y):shortest=1,$($filters -join ','),format=yuv420p,setsar=1[v]"
            [IO.File]::WriteAllText((Join-Path $work "$id.ffgraph"), $graph, $utf8)
            Invoke-FFmpeg ($inputs + @('-filter_complex_threads', '2', '-filter_complex_script', "$id.ffgraph",
                '-map', '[v]', '-t', (Number $shot.Seconds)) + $encode + @("$id.mp4")) $id
            $segments += [pscustomobject]@{ Path = "$id.mp4"; Seconds = $shot.Seconds }
            Write-Host "  $($shot.Id): $($source.Kind)"
        }
        $segments += [pscustomobject]@{ Path = "$format-outro.mp4"; Seconds = $outro }

        # A balanced tree bounds each crossfade to two decoders instead of buffering every shot.
        $round = 0
        while ($segments.Count -gt 1) {
            $next = @()
            for ($i = 0; $i -lt $segments.Count; $i += 2) {
                if ($i + 1 -eq $segments.Count) { $next += $segments[$i]; continue }
                $a = $segments[$i]; $b = $segments[$i + 1]
                $id = "$format-join-$round-$i"
                $seconds = $a.Seconds + $b.Seconds - $fade
                $graph = "[0:v]settb=AVTB,setpts=PTS-STARTPTS[a];[1:v]settb=AVTB,setpts=PTS-STARTPTS[b];" +
                    "[a][b]xfade=transition=fade:duration=$(Number $fade):offset=$(Number ($a.Seconds - $fade)),format=yuv420p[v]"
                Invoke-FFmpeg (@('-i', $a.Path, '-i', $b.Path, '-filter_complex_threads', '2', '-filter_complex', $graph,
                    '-map', '[v]', '-t', (Number $seconds)) + $encode + @("$id.mp4")) $id
                $next += [pscustomobject]@{ Path = "$id.mp4"; Seconds = $seconds }
            }
            $segments = $next
            $round++
        }
        Copy-Item -LiteralPath $segments[0].Path -Destination "$format-picture.mp4" -Force
    }

    Write-Host 'Synthesizing the ambient bed and normalizing loudness'
    $length = Number $duration
    $endFade = Number ($duration - 3)
    $left = '(0.08*sin(2*PI*110*t)+0.045*sin(2*PI*164.8138*t)+0.035*sin(2*PI*220*t)+0.025*sin(2*PI*277.1826*t))*(0.75+0.25*sin(2*PI*0.065*t))'
    $right = '(0.08*sin(2*PI*110*t)+0.045*sin(2*PI*164.8138*t+0.2)+0.035*sin(2*PI*220*t+0.3)+0.025*sin(2*PI*329.6276*t))*(0.75+0.25*sin(2*PI*0.065*t+0.2))'
    $audio = "aevalsrc='$left|$right':s=48000:d=$length,lowpass=f=2400,aecho=0.8:0.7:400|800:0.25|0.12[bed];" +
        "aevalsrc='if(between(t,2.1,$(Number ($duration - 0.8))),0.25*sin(2*PI*70*t),0)':s=48000:d=$length[cue];" +
        "[bed][cue]sidechaincompress=threshold=0.02:ratio=4:attack=120:release=450:makeup=1," +
        "atrim=duration=$length,afade=t=in:d=1.5,afade=t=out:st=${endFade}:d=3[a]"
    Invoke-FFmpeg @('-filter_complex', $audio, '-map', '[a]', '-c:a', 'pcm_s24le', '-ar', '48000', 'music-raw.wav') 'music-synthesis'
    Invoke-FFmpeg @('-i', 'music-raw.wav', '-af', 'loudnorm=I=-20:TP=-2:LRA=7:print_format=json', '-f', 'null', '-') 'loudness-analysis'
    $log = Get-Content -LiteralPath 'loudness-analysis.log' -Raw
    $match = [regex]::Match($log, '(?s)\{\s*"input_i".*?\}')
    if (-not $match.Success) { throw 'FFmpeg did not return loudness measurements.' }
    $levels = $match.Value | ConvertFrom-Json
    $normalize = "loudnorm=I=-20:TP=-2:LRA=7:measured_I=$($levels.input_i):measured_TP=$($levels.input_tp):" +
        "measured_LRA=$($levels.input_lra):measured_thresh=$($levels.input_thresh):offset=$($levels.target_offset):linear=true:print_format=json"
    Invoke-FFmpeg @('-i', 'music-raw.wav', '-af', $normalize, '-ar', '48000', '-c:a', 'pcm_s24le', 'music.wav') 'loudness-normalized'

    foreach ($format in @('16x9', '9x16')) {
        $final = Join-Path $output "demo-$format.mp4"
        Invoke-FFmpeg @('-i', "$format-picture.mp4", '-i', 'music.wav', '-map', '0:v:0', '-map', '1:a:0',
            '-c:v', 'copy', '-c:a', 'aac', '-b:a', '192k', '-ar', '48000', '-t', $length,
            '-movflags', '+faststart', '-metadata', 'title=RUBYLIGHT - Your games. Your screen. Your stream.', $final) "$format-mux"
        $media = Get-Media $final
        $video = @($media.streams | Where-Object codec_type -eq 'video')[0]
        $audioStream = @($media.streams | Where-Object codec_type -eq 'audio')[0]
        $expectedW = 1920; $expectedH = 1080
        if ($format -eq '9x16') { $expectedW = 1080; $expectedH = 1920 }
        $actualDuration = [double]::Parse($media.format.duration, $culture)
        if ($video.codec_name -ne 'h264' -or $video.pix_fmt -ne 'yuv420p' -or $video.r_frame_rate -ne '30/1' -or
                $audioStream.codec_name -ne 'aac' -or $audioStream.channels -ne 2 -or $audioStream.sample_rate -ne '48000' -or
                $video.width -ne $expectedW -or $video.height -ne $expectedH -or
                [Math]::Abs($actualDuration - $duration) -gt 0.1) { throw "Export validation failed: $final" }
        [IO.File]::WriteAllText((Join-Path $output "demo-$format.ffprobe.json"), ($media | ConvertTo-Json -Depth 12), $utf8)
        Invoke-FFmpeg @('-ss', '1', '-i', $final, '-frames:v', '1', '-update', '1', (Join-Path $output "poster-$format.png")) "$format-poster"
        for ($i = 0; $i -lt 8; $i++) {
            $time = 0.8 + ($actualDuration - 1.6) * $i / 7
            $name = 'demo-{0}-{1:00}.png' -f $format, ($i + 1)
            Invoke-FFmpeg @('-ss', (Number $time), '-i', $final, '-frames:v', '1', '-update', '1', (Join-Path $frames $name)) "$format-frame-$i"
        }
        Invoke-FFmpeg @('-i', $final, '-vn', '-af', 'loudnorm=I=-20:TP=-2:LRA=7:print_format=json', '-f', 'null', '-') "$format-audio-check"
        Write-Host "Exported $final and eight review frames"
    }
} finally { Pop-Location }
