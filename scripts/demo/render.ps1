param([switch]$Storyboard)

$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$output = Join-Path $root 'out/demo'
$work = Join-Path $output 'work'
$frames = Join-Path $output 'frames-v3'
$utf8 = New-Object Text.UTF8Encoding($false)
$culture = [Globalization.CultureInfo]::InvariantCulture
$fps = 30
$intro = 3.0
$outro = 5.0
$introFade = 0.5
$outroFade = 0.8
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
        [string]$Color = $white, [string]$Font = 'bold') {
    [IO.File]::WriteAllText((Join-Path $work "$Name.txt"), $Text, $utf8)
    return "drawtext=fontfile=font-$Font.ttf:textfile=$Name.txt:expansion=none:fontsize=${Size}:fontcolor=0x${Color}:x=${X}:y=$Y"
}

function Text-Lines([string]$Name, [string[]]$Lines, [int]$Size, [string]$X, [int]$Y,
        [string]$Color = $white, [string]$Font = 'bold') {
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

# The layout of one shot: where the phone goes and where each piece of text sits, per output format and screen shape.
function Get-Layout([bool]$Vertical, [bool]$Landscape, [int]$SourceW, [int]$SourceH) {
    if ($Vertical) {
        # 9:16 safe areas: wordmark and headline below y=290, captions end by y=1440, nothing in the right 120 px.
        $l = [ordered]@{ Width = 1080; Height = 1920; BrandX = 124; BrandY = 292; Tile = 48; BrandFont = 34
            CountX = 'w-text_w-124'; CountY = 303; HeadingY = 380; BarY = 464; CaptionY = 1378; Right = 956 }
        # Landscape phones stay inside the 120 px side margins (x 120 to 960) so the cut survives platform UI.
        if ($Landscape) { $l.MaxW = 840; $l.MaxH = 540; $l.CenterY = 905 } else { $l.MaxW = 600; $l.MaxH = 830; $l.CenterY = 925 }
    } else {
        $l = [ordered]@{ Width = 1920; Height = 1080; BrandX = 108; BrandY = 58; Tile = 46; BrandFont = 34
            CountX = 'w-text_w-108'; CountY = 66; HeadingY = 404; BarY = 372; CaptionY = 960; Right = 1812 }
        if ($Landscape) { $l.MaxW = 1240; $l.MaxH = 560; $l.CenterY = 548; $l.HeadingY = 150; $l.BarY = 232 }
        else { $l.MaxW = 910; $l.MaxH = 780; $l.CenterY = 500 }
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
        # Portrait or landscape 9:20 phone captures at any scale (capture-video.ps1 records half resolution).
        $aspect = [double]$video.width / [double]$video.height
        if ([Math]::Abs($aspect - 0.45) -gt 0.005 -and [Math]::Abs($aspect - 2.2222) -gt 0.01) { throw "Clip must be a 9:20 phone capture (portrait or landscape): $path" }
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
    $encode = @('-an', '-c:v', 'libx264', '-preset', 'medium', '-crf', '18', '-pix_fmt', 'yuv420p',
        '-r', "$fps", '-threads', '2', '-video_track_timescale', '30000', '-movflags', '+faststart')
    $starts = @{}
    foreach ($format in @('16x9', '9x16')) {
        $vertical = $format -eq '9x16'
        $width = 1920; $height = 1080
        if ($vertical) { $width = 1080; $height = 1920 }
        Write-Host "Rendering $format ($duration seconds)"
        $segments = @()

        # Title cards are fully drawn by GDI+ (centred with measured text widths), so ffmpeg only fades them.
        foreach ($card in @('intro', 'outro')) {
            $id = "$format-$card"
            New-TitleCard "$id.png" $width $height $card $vertical $tile
            if ($card -eq 'intro') {
                $fade = "fade=t=in:st=0:d=$(Number $introFade)"; $seconds = $intro
            } else {
                $fade = "fade=t=out:st=$(Number ($outro - $outroFade)):d=$(Number $outroFade)"; $seconds = $outro
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
                $inputs += @('-i', $source.Path)
                $crop = ''
                if ($shot.Crop -ne 'full') { $crop = "$($shot.Crop)," }
                $padW = [int]([Math]::Ceiling($layout.ScreenW * 2.08 / 2) * 2)
                $padH = [int]([Math]::Ceiling($layout.ScreenH * 2.08 / 2) * 2)
                $count = $shot.Seconds * $fps
                $zoom = "1+0.03*on/($count-1)"
                if ($index % 2 -eq 1) { $zoom = "1.03-0.03*on/($count-1)" }
                $screen = "[2:v]${crop}scale=$($layout.ScreenW * 2):$($layout.ScreenH * 2):force_original_aspect_ratio=decrease:flags=lanczos," +
                    "pad=${padW}:${padH}:(ow-iw)/2:(oh-ih)/2:color=0x111A1F," +
                    "zoompan=z='$zoom':x='iw/2-iw/zoom/2':y='ih/2-ih/zoom/2':d=${count}:s=$($layout.ScreenW)x$($layout.ScreenH):fps=${fps},setsar=1[screen]"
            } else {
                $inputs += @('-ss', '1', '-i', $source.Path)
                $screen = "[2:v]trim=duration=$($shot.Seconds),setpts=PTS-STARTPTS,fps=${fps}," +
                    "scale=$($layout.ScreenW):$($layout.ScreenH):flags=lanczos,setsar=1[screen]"
            }
            $filters = @()
            $filters += Text-Filter "$id-count" ('{0:00} / {1:00}' -f ($index + 1), $shots.Count) 28 $layout.CountX $layout.CountY $muted 'regular'
            $headingLines = $shot.Heading.Split('~')
            if ($vertical -or $landscape) {
                $filters += Text-Filter "$id-heading" $shot.Heading.Replace('~', ' ') 52 '(w-text_w)/2' $layout.HeadingY
            } else {
                $filters += Text-Lines "$id-heading" $headingLines 56 "$($layout.BrandX)" $layout.HeadingY
            }
            $calloutLines = $shot.Callout.Split('~')
            if ($vertical -and $landscape) {
                $filters += Text-Lines "$id-callout" $calloutLines 34 '(w-text_w)/2' ($layout.Y + $layout.ScreenH + 70) $muted 'regular'
            } elseif ($landscape) {
                $filters += Text-Filter "$id-callout" ($calloutLines -join '  ·  ') 30 '(w-text_w)/2' ($layout.Y + $layout.ScreenH + 38) $muted 'regular'
            } elseif (-not $vertical) {
                $filters += Text-Lines "$id-callout" $calloutLines 27 '1450' 446 $muted 'regular'
            }
            $captionLines = @(Wrap-Caption $shot.Caption)
            $captionSize = $(if ($vertical) { 44 } else { 42 })
            $captionTop = $layout.CaptionY - ($captionLines.Count - 1) * ($captionSize + 16)
            $filters += Text-Lines "$id-caption" $captionLines $captionSize '(w-text_w)/2' $captionTop
            $graph = "$screen;[0:v][screen]overlay=$($layout.X):$($layout.Y):shortest=1[phone];[phone][1:v]overlay=0:0:shortest=1," +
                "$($filters -join ','),format=yuv420p,setsar=1[v]"
            [IO.File]::WriteAllText((Join-Path $work "$id.ffgraph"), $graph, $utf8)
            Invoke-FFmpeg ($inputs + @('-filter_complex_threads', '2', '-/filter_complex', "$id.ffgraph",
                '-map', '[v]', '-t', (Number $shot.Seconds)) + $encode + @("$id.mp4")) $id
            $segments += [pscustomobject]@{ Name = $shot.Id; Path = "$id.mp4"; Seconds = $shot.Seconds }
            Write-Host "  $($shot.Id): $($source.Kind)"
        }
        $segments += [pscustomobject]@{ Name = 'outro'; Path = "$format-outro.mp4"; Seconds = $outro }

        # Hard cuts: every segment is encoded with identical settings, so they join losslessly. (A crossfade would
        # leave a few frames of doubled captions and UI text.) Record where each shot starts for the review frames.
        $at = 0.0
        $mid = @{}
        foreach ($segment in $segments) { $mid[$segment.Name] = $at; $at += $segment.Seconds }
        $starts[$format] = $mid
        $list = ($segments | ForEach-Object { "file '$($_.Path)'" }) -join "`n"
        [IO.File]::WriteAllText((Join-Path $work "$format-concat.txt"), $list, $utf8)
        Invoke-FFmpeg @('-f', 'concat', '-safe', '0', '-i', "$format-concat.txt", '-c', 'copy', "$format-picture.mp4") "$format-join"
    }

    # Start times of every segment, for the README GIF and the review frames.
    [IO.File]::WriteAllText((Join-Path $output 'segments.json'), ($starts | ConvertTo-Json -Depth 4), $utf8)

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

    # Ten review frames per video: both title cards and a frame of every shot, including the stream shots (the part of
    # the picture most likely to collide with the sample's own text), the scrolled PC settings and stream menu, and
    # picture-in-picture.
    $review = @(@('intro', 1.8), @('01-launch', 2.0), @('14-pc-settings', 5.0), @('04-library', 2.5), @('05-stream', 3.5),
        @('06-compact', 3.0), @('15-stream-menu', 5.0), @('09-upscaling', 2.5), @('12-pip', 5.3), @('outro', 2.5))
    foreach ($format in @('16x9', '9x16')) {
        $final = Join-Path $output "demo-$format.mp4"
        Invoke-FFmpeg @('-i', "$format-picture.mp4", '-i', 'music.wav', '-map', '0:v:0', '-map', '1:a:0',
            '-c:v', 'copy', '-c:a', 'aac', '-b:a', '192k', '-ar', '48000', '-t', $length,
            '-movflags', '+faststart', '-metadata', 'title=NIGHTJAR - Your games. Your screen. Your stream.', $final) "$format-mux"
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
        Invoke-FFmpeg @('-ss', '1.8', '-i', $final, '-frames:v', '1', '-update', '1', (Join-Path $output "poster-$format.png")) "$format-poster"
        $number = 0
        foreach ($item in $review) {
            $number++
            $time = $starts[$format][$item[0]] + $item[1]
            $name = 'demo-{0}-{1:00}-{2}.png' -f $format, $number, $item[0]
            Invoke-FFmpeg @('-ss', (Number $time), '-i', $final, '-frames:v', '1', '-update', '1', (Join-Path $frames $name)) "$format-frame-$number"
        }
        Invoke-FFmpeg @('-i', $final, '-vn', '-af', 'loudnorm=I=-20:TP=-2:LRA=7:print_format=json', '-f', 'null', '-') "$format-audio-check"
        Write-Host "Exported $final and $($review.Count) review frames"
    }
} finally { Pop-Location; $tile.Dispose() }
