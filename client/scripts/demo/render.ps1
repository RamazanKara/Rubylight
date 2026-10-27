param([switch]$Storyboard)

$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$output = Join-Path $root 'out/demo'
$work = Join-Path $output 'work'
$frames = Join-Path $output 'frames'
$utf8 = New-Object Text.UTF8Encoding($false)
$culture = [Globalization.CultureInfo]::InvariantCulture
$fps = 60
$intro = 3.0
$outro = 5.0
$introFade = 0.5
$outroFade = 0.8
# The title cards and the first and last shot meet through the night background (0.3 s out, 0.3 s in), so their text
# never overlaps. Shots meet with hard cuts; each shot's heading, callout and caption fade in over 0.3 s.
$dip = 0.3
$textFade = 0.3
# The shot whose frame becomes the poster, and the time into it: the stream, with the compact overlay and the game moving.
$posterShot = '05-stream'
$posterAt = 4.2
$repoUrl = 'github.com/RamazanKara/nightjar'
$tilePath = Join-Path $PSScriptRoot 'assets/nightjar-tile.png'

# Palette: the Nightjar brand, an amber moon (#F2A93B) on flat night (#14161F) with ink text (#F4EFE6), as in the
# README banner; secondary text uses the app's night on-surface roles. No gradients or glow.
$accent = '#F2A93B'
$night = '#14161F'
$white = 'F4EFE6'
$muted = 'C1C4D2'
$soft = 'E3E4EE'

function Find-FFmpeg {
    $command = Get-Command ffmpeg.exe -ErrorAction SilentlyContinue
    if ($command) { return $command.Source }
    $winget = Join-Path $env:LOCALAPPDATA 'Microsoft/WinGet'
    $link = Join-Path $winget 'Links/ffmpeg.exe'
    if (Test-Path -LiteralPath $link) {
        $target = (Get-Item -LiteralPath $link).Target
        if ($target -and (Test-Path -LiteralPath $target)) { return $target }
        return $link
    }
    $binary = Get-ChildItem -Path (Join-Path $winget 'Packages/Gyan.FFmpeg_*/*/bin/ffmpeg.exe') -ErrorAction SilentlyContinue |
        Sort-Object FullName | Select-Object -Last 1
    if ($binary) { return $binary.FullName }
    throw 'FFmpeg is not accessible. Open a new shell with the installed FFmpeg bin directory on PATH.'
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
        [string]$Color = $white, [string]$Font = 'bold', [switch]$Fade) {
    [IO.File]::WriteAllText((Join-Path $work "$Name.txt"), $Text, $utf8)
    # drawtext evaluates alpha every frame, so the fade is as smooth as the video.
    $alpha = $(if ($Fade) { ":alpha='min(1,t/$(Number $textFade))'" } else { '' })
    return "drawtext=fontfile=font-$Font.ttf:textfile=$Name.txt:expansion=none:fontsize=${Size}:fontcolor=0x${Color}:x=${X}:y=$Y$alpha"
}

function Text-Lines([string]$Name, [string[]]$Lines, [int]$Size, [string]$X, [int]$Y,
        [string]$Color = $white, [string]$Font = 'bold', [switch]$Fade) {
    $filters = @()
    for ($line = 0; $line -lt $Lines.Count; $line++) {
        $filters += Text-Filter "$Name-$line" $Lines[$line] $Size $X ($Y + $line * ($Size + 16)) $Color $Font -Fade:$Fade
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

# ---- GDI+ drawing for the static artwork: backdrops, brand lockups, title cards, phone frames -----------------------

function Hex([string]$Value) { return [Drawing.ColorTranslator]::FromHtml($Value) }

function Round-Path([int]$X, [int]$Y, [int]$Width, [int]$Height, [int]$Radius) {
    $path = New-Object Drawing.Drawing2D.GraphicsPath
    $d = $Radius * 2
    $path.AddArc($X, $Y, $d, $d, 180, 90)
    $path.AddArc($X + $Width - $d, $Y, $d, $d, 270, 90)
    $path.AddArc($X + $Width - $d, $Y + $Height - $d, $d, $d, 0, 90)
    $path.AddArc($X, $Y + $Height - $d, $d, $d, 90, 90)
    $path.CloseFigure()
    return $path
}

function Round-Rectangle([Drawing.Graphics]$Graphics, [Drawing.Brush]$Brush,
        [int]$X, [int]$Y, [int]$Width, [int]$Height, [int]$Radius) {
    $path = Round-Path $X $Y $Width $Height $Radius
    try { $Graphics.FillPath($Brush, $path) } finally { $path.Dispose() }
}

function New-Font([int]$Pixels, [string]$Style = 'Bold') {
    return New-Object Drawing.Font('Segoe UI', [single]$Pixels, [Drawing.FontStyle]::$Style, [Drawing.GraphicsUnit]::Pixel)
}

function Measure-Text([Drawing.Graphics]$Graphics, [string]$Text, [Drawing.Font]$Font) {
    return $Graphics.MeasureString($Text, $Font, 4000, [Drawing.StringFormat]::GenericTypographic).Width
}

function Draw-Text([Drawing.Graphics]$Graphics, [string]$Text, [Drawing.Font]$Font, [Drawing.Brush]$Brush, [double]$X, [double]$Y) {
    $Graphics.DrawString($Text, $Font, $Brush, [single]$X, [single]$Y, [Drawing.StringFormat]::GenericTypographic)
}

function Draw-CenteredText([Drawing.Graphics]$Graphics, [string]$Text, [Drawing.Font]$Font, [Drawing.Brush]$Brush,
        [double]$CenterX, [double]$Y) {
    $width = Measure-Text $Graphics $Text $Font
    Draw-Text $Graphics $Text $Font $Brush ($CenterX - $width / 2) $Y
}

function New-Card([string]$Name, [int]$Width, [int]$Height, [scriptblock]$Draw) {
    $bitmap = New-Object Drawing.Bitmap($Width, $Height)
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    try {
        $graphics.SmoothingMode = [Drawing.Drawing2D.SmoothingMode]::AntiAlias
        $graphics.TextRenderingHint = [Drawing.Text.TextRenderingHint]::AntiAliasGridFit
        $graphics.InterpolationMode = [Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        & $Draw $graphics $Width $Height
        $bitmap.Save((Join-Path $work $Name), [Drawing.Imaging.ImageFormat]::Png)
    } finally { $graphics.Dispose(); $bitmap.Dispose() }
}

function Draw-Backdrop([Drawing.Graphics]$Graphics, [int]$Width, [int]$Height, [double]$RingX, [double]$RingY, [double]$RingStep) {
    $rect = New-Object Drawing.Rectangle(0, 0, $Width, $Height)
    $fill = New-Object Drawing.SolidBrush((Hex $night))
    # Concentric rings echo the README banner.
    $pen = New-Object Drawing.Pen([Drawing.Color]::FromArgb(54, 242, 169, 59), 2)
    try {
        $Graphics.FillRectangle($fill, $rect)
        for ($ring = 1; $ring -le 5; $ring++) {
            $radius = $ring * $RingStep
            $Graphics.DrawEllipse($pen, [single]($RingX - $radius), [single]($RingY - $radius), [single](2 * $radius), [single](2 * $radius))
        }
    } finally { $pen.Dispose(); $fill.Dispose() }
}

function Draw-Tile([Drawing.Graphics]$Graphics, [Drawing.Image]$Tile, [double]$X, [double]$Y, [double]$Size) {
    $Graphics.DrawImage($Tile, [single]$X, [single]$Y, [single]$Size, [single]$Size)
}

function Draw-BrandRow([Drawing.Graphics]$Graphics, [Drawing.Image]$Tile, [int]$X, [int]$Y, [int]$TileSize, [int]$FontPixels) {
    $font = New-Font $FontPixels
    $brush = New-Object Drawing.SolidBrush((Hex "#$white"))
    try {
        Draw-Tile $Graphics $Tile $X $Y $TileSize
        $height = $Graphics.MeasureString('NIGHTJAR', $font, 4000, [Drawing.StringFormat]::GenericTypographic).Height
        Draw-Text $Graphics 'NIGHTJAR' $font $brush ($X + $TileSize + 16) ($Y + ($TileSize - $height) / 2)
    } finally { $font.Dispose(); $brush.Dispose() }
}

function New-TitleCard([string]$Name, [int]$Width, [int]$Height, [string]$Card, [bool]$Vertical, [Drawing.Image]$Tile) {
    New-Card $Name $Width $Height {
        param($g, $w, $h)
        $whiteBrush = New-Object Drawing.SolidBrush((Hex "#$white"))
        $mutedBrush = New-Object Drawing.SolidBrush((Hex "#$muted"))
        $softBrush = New-Object Drawing.SolidBrush((Hex "#$soft"))
        $accentBrush = New-Object Drawing.SolidBrush((Hex $accent))
        $nightBrush = New-Object Drawing.SolidBrush((Hex $night))
        $fonts = @()
        try {
            if ($Card -eq 'intro') {
                if ($Vertical) {
                    Draw-Backdrop $g $w $h ($w / 2) ($h * 0.55) 170
                    $word = New-Font 116; $tag = New-Font 46 'Regular'; $fonts += $word, $tag
                    Draw-Tile $g $Tile (($w - 200) / 2) 590 200
                    Draw-CenteredText $g 'NIGHTJAR' $word $whiteBrush ($w / 2) 825
                    Draw-CenteredText $g 'Your games. Your screen.' $tag $softBrush ($w / 2) 1015
                    Draw-CenteredText $g 'Your stream.' $tag $softBrush ($w / 2) 1080
                } else {
                    Draw-Backdrop $g $w $h ($w * 0.8) ($h / 2) 150
                    $word = New-Font 150; $tag = New-Font 48 'Regular'; $fonts += $word, $tag
                    $wordWidth = Measure-Text $g 'NIGHTJAR' $word
                    $tagWidth = Measure-Text $g 'Your games. Your screen. Your stream.' $tag
                    $tileSize = 190; $gap = 56
                    $left = ($w - ($tileSize + $gap + [Math]::Max($wordWidth, $tagWidth))) / 2
                    $top = 330
                    Draw-Tile $g $Tile $left ($top + 20) $tileSize
                    Draw-Text $g 'NIGHTJAR' $word $whiteBrush ($left + $tileSize + $gap) ($top - 30)
                    Draw-Text $g 'Your games. Your screen. Your stream.' $tag $softBrush ($left + $tileSize + $gap + 4) ($top + 180)
                }
            } else {
                if ($Vertical) {
                    Draw-Backdrop $g $w $h ($w / 2) ($h * 0.6) 170
                    $word = New-Font 84; $big = New-Font 80; $get = New-Font 44; $dl = New-Font 44; $url = New-Font 31 'Regular'
                    $fonts += $word, $big, $get, $dl, $url
                    Draw-Tile $g $Tile (($w - 132) / 2) 490 132
                    Draw-CenteredText $g 'NIGHTJAR' $word $whiteBrush ($w / 2) 650
                    Draw-CenteredText $g 'Start your stream.' $big $whiteBrush ($w / 2) 810
                    $pillW = [int]((Measure-Text $g 'Get NIGHTJAR' $get) + 120); $pillH = 100
                    Round-Rectangle $g $accentBrush ([int](($w - $pillW) / 2)) 960 $pillW $pillH 50
                    Draw-CenteredText $g 'Get NIGHTJAR' $get $nightBrush ($w / 2) 982
                    Draw-CenteredText $g 'Download on GitHub' $dl $whiteBrush ($w / 2) 1110
                    Draw-CenteredText $g $repoUrl $url $mutedBrush ($w / 2) 1190
                } else {
                    Draw-Backdrop $g $w $h ($w * 0.82) ($h / 2) 150
                    $word = New-Font 78; $big = New-Font 108; $get = New-Font 44; $dl = New-Font 44; $url = New-Font 34 'Regular'
                    $fonts += $word, $big, $get, $dl, $url
                    $wordWidth = Measure-Text $g 'NIGHTJAR' $word
                    $tileSize = 112; $gap = 28
                    $left = ($w - ($tileSize + $gap + $wordWidth)) / 2
                    Draw-Tile $g $Tile $left 150 $tileSize
                    Draw-Text $g 'NIGHTJAR' $word $whiteBrush ($left + $tileSize + $gap) 168
                    Draw-CenteredText $g 'Start your stream.' $big $whiteBrush ($w / 2) 330
                    $pillW = [int]((Measure-Text $g 'Get NIGHTJAR' $get) + 140); $pillH = 104
                    Round-Rectangle $g $accentBrush ([int](($w - $pillW) / 2)) 530 $pillW $pillH 52
                    Draw-CenteredText $g 'Get NIGHTJAR' $get $nightBrush ($w / 2) 554
                    Draw-CenteredText $g 'Download on GitHub' $dl $whiteBrush ($w / 2) 690
                    Draw-CenteredText $g $repoUrl $url $mutedBrush ($w / 2) 765
                }
            }
        } finally {
            foreach ($font in $fonts) { $font.Dispose() }
            $whiteBrush.Dispose(); $mutedBrush.Dispose(); $softBrush.Dispose(); $accentBrush.Dispose(); $nightBrush.Dispose()
        }
    }
}

function Assert-Fits([string]$Text, [int]$Pixels, [string]$Style, [double]$MaxWidth, [string]$What) {
    # GDI+ measures Segoe UI a few pixels differently from FreeType's drawtext; the margins leave room for that.
    $bitmap = New-Object Drawing.Bitmap(1, 1)
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    $font = New-Font $Pixels $Style
    try {
        $width = Measure-Text $graphics $Text $font
        if ($width -gt $MaxWidth) { throw "$What is too wide ($([int]$width) of $([int]$MaxWidth) px): $Text" }
    } finally { $font.Dispose(); $graphics.Dispose(); $bitmap.Dispose() }
}

# The layout of one shot: where the phone goes and where each piece of text sits, per output format and screen shape.
# The phone is as large as the text around it allows; the captures are 648x1440 (or 1440x648), so the screen is only
# ever scaled down.
function Get-Layout([bool]$Vertical, [bool]$Landscape, [int]$SourceW, [int]$SourceH) {
    if ($Vertical) {
        # 9:16 safe areas: wordmark and headline below y=290, captions end by y=1440, nothing in the right 120 px.
        $l = [ordered]@{ Width = 1080; Height = 1920; BrandX = 124; BrandY = 292; Tile = 48; BrandFont = 34
            CountX = 'w-text_w-124'; CountY = 303; HeadingY = 380; BarY = 464; CaptionY = 1378; CaptionSize = 44
            CalloutSize = 34; Right = 956 }
        # Landscape phones stay inside the 120 px side margins (x 120 to 960) so the cut survives platform UI.
        if ($Landscape) { $l.MaxW = 840; $l.MaxH = 540; $l.CenterY = 905 } else { $l.MaxW = 600; $l.MaxH = 820; $l.CenterY = 925 }
    } else {
        $l = [ordered]@{ Width = 1920; Height = 1080; BrandX = 108; BrandY = 58; Tile = 46; BrandFont = 34
            CountX = 'w-text_w-108'; CountY = 66; CaptionSize = 42; CalloutSize = 30; Right = 1812 }
        if ($Landscape) {
            # Heading above the phone, caption and the callout on one line below it.
            $l.MaxW = 1400; $l.MaxH = 630; $l.CenterY = 520; $l.HeadingY = 80; $l.BarY = 152; $l.CaptionY = 893
            $l.CalloutY = 962
        } else {
            # Heading on the left, callout on the right, caption below the phone.
            $l.MaxW = 900; $l.MaxH = 860; $l.CenterY = 505; $l.HeadingY = 404; $l.BarY = 372; $l.CaptionY = 980
            $l.CalloutX = 1300; $l.CalloutY = 446
        }
    }
    $scale = [Math]::Min($l.MaxW / $SourceW, $l.MaxH / $SourceH)
    $l.ScreenW = [int]([Math]::Floor($SourceW * $scale / 2) * 2)
    $l.ScreenH = [int]([Math]::Floor($SourceH * $scale / 2) * 2)
    $l.X = [int](($l.Width - $l.ScreenW) / 2)
    $l.Y = [int]($l.CenterY - $l.ScreenH / 2)
    return [pscustomobject]$l
}

function New-ShotArt([string]$Id, $Layout, [bool]$Vertical, [bool]$Landscape, [Drawing.Image]$Tile) {
    # Backdrop (flat night, rings, brand row, accent bar, phone body) and a separate bezel with rounded display corners.
    New-Card "$Id-bg.png" $Layout.Width $Layout.Height {
        param($g, $w, $h)
        Draw-Backdrop $g $w $h ($Layout.X + $Layout.ScreenW / 2) ($Layout.Y + $Layout.ScreenH / 2) 200
        Draw-BrandRow $g $Tile $Layout.BrandX $Layout.BrandY $Layout.Tile $Layout.BrandFont
        $accentBrush = New-Object Drawing.SolidBrush((Hex $accent))
        $rim = New-Object Drawing.SolidBrush((Hex '#3A3D4C'))
        $shadow = New-Object Drawing.SolidBrush([Drawing.Color]::FromArgb(80, 0, 0, 0))
        try {
            # Heading accent bar: centred under a centred heading, otherwise above the left-aligned one.
            $centered = $Vertical -or $Landscape
            $barX = $(if ($centered) { [int](($w - 68) / 2) } else { $Layout.BrandX })
            $g.FillRectangle($accentBrush, $barX, $Layout.BarY, 68, 4)
            $x = $Layout.X; $y = $Layout.Y; $pw = $Layout.ScreenW; $ph = $Layout.ScreenH
            Round-Rectangle $g $shadow ($x - 26) ($y - 4) ($pw + 52) ($ph + 40) 40
            Round-Rectangle $g $rim ($x - 18) ($y - 18) ($pw + 36) ($ph + 36) 34
            $g.FillRectangle($rim, ($x + $pw + 18), ($y + 70), 4, 60)
        } finally { $accentBrush.Dispose(); $rim.Dispose(); $shadow.Dispose() }
    }
    New-Card "$Id-bezel.png" $Layout.Width $Layout.Height {
        param($g, $w, $h)
        $x = $Layout.X; $y = $Layout.Y; $pw = $Layout.ScreenW; $ph = $Layout.ScreenH
        $g.Clear([Drawing.Color]::FromArgb(0, 0, 0, 0))
        $black = New-Object Drawing.SolidBrush((Hex '#07090C'))
        $hole = Round-Path $x $y $pw $ph 26
        $ring = Round-Path ($x - 16) ($y - 16) ($pw + 32) ($ph + 32) 32
        $region = New-Object Drawing.Region($ring)
        try {
            $region.Exclude($hole)
            $g.FillRegion($black, $region)
        } finally { $region.Dispose(); $hole.Dispose(); $ring.Dispose(); $black.Dispose() }
    }
}

# ---- Manifest and sources ------------------------------------------------------------------------------------------

$shots = @(Get-Content -LiteralPath (Join-Path $root 'docs/demo/shotlist.md') | ForEach-Object {
    if ($_ -match '^\| \d{2}-') {
        $cells = $_.Split('|').Trim()
        [pscustomobject]@{
            Id = $cells[1]; Seconds = [int]$cells[2]; Caption = $cells[3]
            Heading = $cells[4]; Callout = $cells[5]; Still = $cells[6]; Crop = $cells[7]
        }
    }
})
if ($shots.Count -lt 6) { throw 'Expected the shots table in docs/demo/shotlist.md.' }
$duration = $intro + $outro + ($shots | Measure-Object Seconds -Sum).Sum
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
if (-not (Test-Path -LiteralPath $tilePath)) { throw "Missing brand tile: $tilePath" }
Add-Type -AssemblyName System.Drawing
$tile = [Drawing.Image]::FromFile($tilePath)
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
        # Portrait or landscape 9:20 phone captures at any scale (capture-video.ps1 records 0.6 of the screen size), at 60 fps.
        $aspect = [double]$video.width / [double]$video.height
        if ([Math]::Abs($aspect - 0.45) -gt 0.005 -and [Math]::Abs($aspect - 2.2222) -gt 0.01) { throw "Clip must be a 9:20 phone capture (portrait or landscape): $path" }
        if ($video.r_frame_rate -ne "$fps/1") { throw "Clip is not $fps fps; capture it again with capture-video.ps1: $path" }
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

# ---- Pictures ------------------------------------------------------------------------------------------------------

# Relative filter paths avoid Windows drive-colon and apostrophe escaping in drawtext.
Push-Location $work
try {
    # Every segment is encoded alike, so the segments join without re-encoding. Two-second GOPs keep seeking quick.
    $encode = @('-an', '-c:v', 'libx264', '-preset', 'slow', '-crf', '17', '-profile:v', 'high', '-pix_fmt', 'yuv420p',
        '-r', "$fps", '-g', "$(2 * $fps)", '-threads', '4', '-video_track_timescale', "$($fps * 1000)", '-movflags', '+faststart')
    $starts = @{}
    foreach ($format in @('16x9', '9x16')) {
        $vertical = $format -eq '9x16'
        $width = 1920; $height = 1080
        if ($vertical) { $width = 1080; $height = 1920 }
        Write-Host "Rendering $format ($duration seconds at $fps fps)"
        $segments = @()
        $nightHex = '0x' + $night.TrimStart('#')

        # Title cards are fully drawn by GDI+ (centred with measured text widths), so ffmpeg only fades them.
        foreach ($card in @('intro', 'outro')) {
            $id = "$format-$card"
            New-TitleCard "$id.png" $width $height $card $vertical $tile
            if ($card -eq 'intro') {
                $seconds = $intro
                $fade = "fade=t=in:st=0:d=$(Number $introFade),fade=t=out:st=$(Number ($intro - $dip)):d=$(Number $dip):color=$nightHex"
            } else {
                $seconds = $outro
                $fade = "fade=t=in:st=0:d=$(Number $dip):color=$nightHex,fade=t=out:st=$(Number ($outro - $outroFade)):d=$(Number $outroFade)"
            }
            Invoke-FFmpeg (@('-loop', '1', '-framerate', "$fps", '-i', "$id.png", '-vf', "$fade,setsar=1,format=yuv420p",
                '-t', (Number $seconds)) + $encode + @("$id.mp4")) $id
        }
        $segments += [pscustomobject]@{ Name = 'intro'; Path = "$format-intro.mp4"; Seconds = $intro }
        for ($index = 0; $index -lt $shots.Count; $index++) {
            $shot = $shots[$index]; $source = $sources[$index]
            $id = "$format-$($shot.Id)"
            $landscape = $source.Width -gt $source.Height
            $layout = Get-Layout $vertical $landscape $source.Width $source.Height
            New-ShotArt $id $layout $vertical $landscape $tile
            $inputs = @('-loop', '1', '-framerate', "$fps", '-i', "$id-bg.png", '-loop', '1', '-framerate', "$fps", '-i', "$id-bezel.png")
            if ($source.Kind -eq 'storyboard') {
                # A still, scaled once and held: no zoom.
                $inputs += @('-loop', '1', '-framerate', "$fps", '-i', $source.Path)
                $crop = ''
                if ($shot.Crop -ne 'full') { $crop = "$($shot.Crop)," }
                $screen = "[2:v]${crop}scale=$($layout.ScreenW):$($layout.ScreenH):flags=lanczos,setsar=1[screen]"
            } else {
                # The 60 fps clip from its one-second lead handle; scaling it down is the only change.
                $inputs += @('-ss', '1', '-i', $source.Path)
                $screen = "[2:v]trim=duration=$($shot.Seconds),setpts=PTS-STARTPTS," +
                    "scale=$($layout.ScreenW):$($layout.ScreenH):flags=lanczos,setsar=1[screen]"
            }
            $filters = @()
            $filters += Text-Filter "$id-count" ('{0:00} / {1:00}' -f ($index + 1), $shots.Count) 28 $layout.CountX $layout.CountY $muted 'regular'
            $headingLines = $shot.Heading.Split('~')
            if ($vertical -or $landscape) {
                $filters += Text-Filter "$id-heading" $shot.Heading.Replace('~', ' ') 52 '(w-text_w)/2' $layout.HeadingY -Fade
            } else {
                $filters += Text-Lines "$id-heading" $headingLines 56 "$($layout.BrandX)" $layout.HeadingY -Fade
            }
            $calloutLines = $shot.Callout.Split('~')
            if ($vertical -and $landscape) {
                $filters += Text-Lines "$id-callout" $calloutLines $layout.CalloutSize '(w-text_w)/2' ($layout.Y + $layout.ScreenH + 70) $muted 'regular' -Fade
            } elseif ($landscape) {
                $filters += Text-Filter "$id-callout" ($calloutLines -join '  ·  ') $layout.CalloutSize '(w-text_w)/2' $layout.CalloutY $muted 'regular' -Fade
            } elseif (-not $vertical) {
                $filters += Text-Lines "$id-callout" $calloutLines $layout.CalloutSize "$($layout.CalloutX)" $layout.CalloutY $muted 'regular' -Fade
            }
            # Text stays inside the frame's margins (for 9:16, also out of the right 120 px that platform buttons cover).
            $textWidth = $(if ($vertical) { 840 } else { 1920 - 2 * 108 })
            foreach ($line in $calloutLines) {
                if ($landscape -and -not $vertical) { Assert-Fits ($calloutLines -join '  ·  ') $layout.CalloutSize 'Regular' $textWidth "$id callout" }
                elseif ($landscape) { Assert-Fits $line $layout.CalloutSize 'Regular' $textWidth "$id callout" }
                elseif (-not $vertical) { Assert-Fits $line $layout.CalloutSize 'Regular' (1812 - $layout.CalloutX) "$id callout" }
            }
            Assert-Fits $shot.Heading.Replace('~', ' ') 52 'Bold' $textWidth "$id heading"
            $captionLines = @(Wrap-Caption $shot.Caption)
            foreach ($line in $captionLines) { Assert-Fits $line $layout.CaptionSize 'Bold' $textWidth "$id caption" }
            $captionTop = $layout.CaptionY - ($captionLines.Count - 1) * ($layout.CaptionSize + 16)
            $filters += Text-Lines "$id-caption" $captionLines $layout.CaptionSize '(w-text_w)/2' $captionTop -Fade
            # The first and last shots meet the title cards through the night background.
            if ($index -eq 0) { $filters += "fade=t=in:st=0:d=$(Number $dip):color=$nightHex" }
            if ($index -eq $shots.Count - 1) {
                $filters += "fade=t=out:st=$(Number ($shot.Seconds - $dip)):d=$(Number $dip):color=$nightHex"
            }
            $graph = "$screen;[0:v][screen]overlay=$($layout.X):$($layout.Y):shortest=1[phone];[phone][1:v]overlay=0:0:shortest=1," +
                "$($filters -join ','),format=yuv420p,setsar=1[v]"
            [IO.File]::WriteAllText((Join-Path $work "$id.ffgraph"), $graph, $utf8)
            Invoke-FFmpeg ($inputs + @('-filter_complex_threads', '4', '-/filter_complex', "$id.ffgraph",
                '-map', '[v]', '-t', (Number $shot.Seconds)) + $encode + @("$id.mp4")) $id
            $segments += [pscustomobject]@{ Name = $shot.Id; Path = "$id.mp4"; Seconds = $shot.Seconds }
            Write-Host "  $($shot.Id): $($source.Kind)"
        }
        $segments += [pscustomobject]@{ Name = 'outro'; Path = "$format-outro.mp4"; Seconds = $outro }

        # Every segment is encoded with identical settings, so they join losslessly. Record where each one starts for
        # the review frames, the README GIF and the smoothness check.
        $at = 0.0
        $mid = [ordered]@{}
        foreach ($segment in $segments) { $mid[$segment.Name] = $at; $at += $segment.Seconds }
        $starts[$format] = $mid
        $list = ($segments | ForEach-Object { "file '$($_.Path)'" }) -join "`n"
        [IO.File]::WriteAllText((Join-Path $work "$format-concat.txt"), $list, $utf8)
        Invoke-FFmpeg @('-f', 'concat', '-safe', '0', '-i', "$format-concat.txt", '-c', 'copy', "$format-picture.mp4") "$format-join"
    }

    # Start times of every segment, for the README GIF and the review frames.
    [IO.File]::WriteAllText((Join-Path $output 'segments.json'), ($starts | ConvertTo-Json -Depth 4), $utf8)

    foreach ($format in @('16x9', '9x16')) {
        # No audio track: the owner adds licensed music when posting the video.
        $final = Join-Path $output "demo-$format.mp4"
        Invoke-FFmpeg @('-i', "$format-picture.mp4", '-map', '0:v:0', '-c:v', 'copy', '-an', '-t', (Number $duration),
            '-movflags', '+faststart', '-metadata', 'title=NIGHTJAR - Your games. Your screen. Your stream.', $final) "$format-mux"
        $media = Get-Media $final
        $video = @($media.streams | Where-Object codec_type -eq 'video')[0]
        $expectedW = 1920; $expectedH = 1080
        if ($format -eq '9x16') { $expectedW = 1080; $expectedH = 1920 }
        $actualDuration = [double]::Parse($media.format.duration, $culture)
        if ($video.codec_name -ne 'h264' -or $video.pix_fmt -ne 'yuv420p' -or $video.r_frame_rate -ne "$fps/1" -or
                @($media.streams).Count -ne 1 -or $video.width -ne $expectedW -or $video.height -ne $expectedH -or
                [Math]::Abs($actualDuration - $duration) -gt 0.1) { throw "Export validation failed: $final" }
        [IO.File]::WriteAllText((Join-Path $output "demo-$format.ffprobe.json"), ($media | ConvertTo-Json -Depth 12), $utf8)

        # The poster: a frame of the stream, game moving and overlay in view, as a lossless PNG and the site's WebP
        # (1280x720 or 720x1280).
        $posterTime = Number ($starts[$format][$posterShot] + $posterAt)
        $posterSize = $(if ($format -eq '9x16') { '720:1280' } else { '1280:720' })
        Invoke-FFmpeg @('-ss', $posterTime, '-i', $final, '-frames:v', '1', '-update', '1', (Join-Path $output "poster-$format.png")) "$format-poster"
        Invoke-FFmpeg @('-ss', $posterTime, '-i', $final, '-frames:v', '1', '-vf', "scale=${posterSize}:flags=lanczos",
            '-c:v', 'libwebp', '-quality', '86', '-compression_level', '6', '-update', '1', (Join-Path $output "poster-$format.webp")) "$format-poster-webp"

        # Review frames: both title cards and one frame of every shot, near its end, where its action has its result.
        $number = 0
        $review = @(@('intro', 1.8)) + @($shots | ForEach-Object { , @($_.Id, ($_.Seconds - 0.6)) }) + @(, @('outro', 2.5))
        foreach ($item in $review) {
            $number++
            $time = $starts[$format][$item[0]] + $item[1]
            $name = 'demo-{0}-{1:00}-{2}.png' -f $format, $number, $item[0]
            Invoke-FFmpeg @('-ss', (Number $time), '-i', $final, '-frames:v', '1', '-update', '1', (Join-Path $frames $name)) "$format-frame-$number"
        }

        # A contact sheet: three frames of every shot (just after the cut, the middle, the end) and the title cards.
        $cells = @()
        foreach ($name in @('intro') + $shots.Id + @('outro')) {
            $length = $(if ($name -eq 'intro') { $intro } elseif ($name -eq 'outro') { $outro } else { ($shots | Where-Object Id -eq $name).Seconds })
            foreach ($fraction in 0.1, 0.5, 0.92) {
                $file = Join-Path $work ('{0}-sheet-{1}-{2}.png' -f $format, $name, $fraction)
                $time = $starts[$format][$name] + $length * $fraction
                Invoke-FFmpeg @('-ss', (Number $time), '-i', $final, '-frames:v', '1', '-update', '1', $file) "$format-sheet"
                $cells += [pscustomobject]@{ Label = '{0} +{1:0.0}s' -f $name, ($length * $fraction); Path = $file }
            }
        }
        $cellW = $(if ($vertical) { 270 } else { 480 })
        $cellH = [int]($cellW * $expectedH / $expectedW)
        $columns = 6
        $rows = [Math]::Ceiling($cells.Count / $columns)
        $sheet = New-Object Drawing.Bitmap(($columns * $cellW), ($rows * ($cellH + 24)))
        $g = [Drawing.Graphics]::FromImage($sheet)
        $labelFont = New-Font 15 'Regular'
        $labelBrush = New-Object Drawing.SolidBrush((Hex '#F2A93B'))
        try {
            $g.Clear((Hex '#2A2D3A'))
            $g.InterpolationMode = [Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
            for ($i = 0; $i -lt $cells.Count; $i++) {
                $x = ($i % $columns) * $cellW; $y = [Math]::Floor($i / $columns) * ($cellH + 24)
                $image = [Drawing.Image]::FromFile($cells[$i].Path)
                try { $g.DrawImage($image, $x, $y + 24, $cellW, $cellH) } finally { $image.Dispose() }
                $g.DrawString($cells[$i].Label, $labelFont, $labelBrush, [single]($x + 4), [single]($y + 3))
            }
            $sheet.Save((Join-Path $output "contact-$format.png"), [Drawing.Imaging.ImageFormat]::Png)
        } finally { $labelFont.Dispose(); $labelBrush.Dispose(); $g.Dispose(); $sheet.Dispose() }

        # Smoothness: FFmpeg's mpdecimate drops frames that repeat the one before; what is left per second of each shot
        # is how many distinct frames it shows. Motion (a scroll, a transition, the game) should keep close to 60.
        $log = Join-Path $work "$format-smoothness.log"
        $ErrorActionPreference = 'Continue'
        & $ffmpeg -hide_banner -nostdin -i $final -vf 'mpdecimate=hi=64*4:lo=64*2:frac=0.2,showinfo' -an -f null - 2> $log
        $ErrorActionPreference = 'Stop'
        $kept = @(Select-String -LiteralPath $log -Pattern 'pts_time:([0-9.]+)' | ForEach-Object {
            [double]::Parse($_.Matches[0].Groups[1].Value, $culture) })
        $report = @("Distinct frames per second ($format, mpdecimate hi=64*4 lo=64*2 frac=0.2)")
        foreach ($segment in $starts[$format].GetEnumerator()) {
            $length = $(if ($segment.Key -eq 'intro') { $intro } elseif ($segment.Key -eq 'outro') { $outro } else { ($shots | Where-Object Id -eq $segment.Key).Seconds })
            $perSecond = @()
            for ($second = 0; $second -lt $length; $second++) {
                $from = $segment.Value + $second; $to = $from + 1
                $perSecond += @($kept | Where-Object { $_ -ge $from - 0.0001 -and $_ -lt $to - 0.0001 }).Count
            }
            $report += '{0,-16} {1}' -f $segment.Key, ($perSecond -join ' ')
        }
        [IO.File]::WriteAllLines((Join-Path $output "smoothness-$format.txt"), [string[]]$report, $utf8)
        $report | ForEach-Object { Write-Host $_ }
        Write-Host "Exported $final, poster, $($review.Count) review frames and a contact sheet"
    }
} finally { Pop-Location; $tile.Dispose() }
