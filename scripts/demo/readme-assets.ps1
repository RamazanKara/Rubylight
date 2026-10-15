param(
    # Repository root whose docs/assets receives the README images (defaults to this checkout).
    [string]$Docs = ([IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))),
    [switch]$SkipGif
)

# Builds the README's phone screenshots (docs/assets/shots) and the animated demo.gif from the demo captures:
# the stills in docs/screenshots/demo and, for the GIF, the rendered 9:16 video (out/demo, or site/assets without a render).
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$stills = Join-Path $root 'docs/screenshots/demo'
$assets = Join-Path $Docs 'docs/assets'
$shots = Join-Path $assets 'shots'
$work = Join-Path $root 'out/demo/work'
$null = New-Item -ItemType Directory -Force $shots, $work
Add-Type -AssemblyName System.Drawing

function Save-Shot([string]$Source, [string]$Name, $Crop = $null, [int]$height = 800) {
    # 360 px wide shot (800 high for a phone, other heights for crops): rounded corners and the thin grey outline the README shots share.
    $width = 360; $radius = 30; $border = 3
    $image = [Drawing.Image]::FromFile($Source)
    $bitmap = New-Object Drawing.Bitmap($width, $height, [Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [Drawing.Graphics]::FromImage($bitmap)
    try {
        $g.SmoothingMode = [Drawing.Drawing2D.SmoothingMode]::AntiAlias
        $g.InterpolationMode = [Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $g.PixelOffsetMode = [Drawing.Drawing2D.PixelOffsetMode]::HighQuality
        $outer = New-Object Drawing.Drawing2D.GraphicsPath
        $d = $radius * 2
        $outer.AddArc(0, 0, $d, $d, 180, 90); $outer.AddArc($width - $d, 0, $d, $d, 270, 90)
        $outer.AddArc($width - $d, $height - $d, $d, $d, 0, 90); $outer.AddArc(0, $height - $d, $d, $d, 90, 90)
        $outer.CloseFigure()
        $brush = New-Object Drawing.SolidBrush([Drawing.ColorTranslator]::FromHtml('#BCC4CC'))
        $g.FillPath($brush, $outer)
        $inner = New-Object Drawing.Drawing2D.GraphicsPath
        $r = $radius - $border; $d = $r * 2
        $iw = $width - 2 * $border; $ih = $height - 2 * $border
        $inner.AddArc($border, $border, $d, $d, 180, 90); $inner.AddArc($border + $iw - $d, $border, $d, $d, 270, 90)
        $inner.AddArc($border + $iw - $d, $border + $ih - $d, $d, $d, 0, 90); $inner.AddArc($border, $border + $ih - $d, $d, $d, 90, 90)
        $inner.CloseFigure()
        $g.SetClip($inner)
        $target = New-Object Drawing.Rectangle($border, $border, $iw, $ih)
        if ($Crop) {
            $region = New-Object Drawing.Rectangle($Crop[0], $Crop[1], $Crop[2], $Crop[3])
            $g.DrawImage($image, $target, $region, [Drawing.GraphicsUnit]::Pixel)
        } else {
            $g.DrawImage($image, $target, (New-Object Drawing.Rectangle(0, 0, $image.Width, $image.Height)), [Drawing.GraphicsUnit]::Pixel)
        }
        $bitmap.Save((Join-Path $shots $Name), [Drawing.Imaging.ImageFormat]::Png)
        Write-Host "Saved shots/$Name"
    } finally { $g.Dispose(); $bitmap.Dispose(); $image.Dispose() }
}

# The four tiles: PCs, library, a landscape stream with the compact overlay, and picture-in-picture. All four are
# the same 360x800 portrait size, so the README shows them as a 2x2 grid on a phone and one row of four on a desktop.
# The stream still is landscape (2400x1080); its tile is a portrait slice of full height (x 160 to 646, 9:20 like the
# phone shots) from the screen's top-left corner, where the compact overlay sits. The whole overlay (text x 173 to 858)
# is wider than any 9:20 slice of the still, so the slice keeps its first two stats at full size, from the health dot to
# "8 ms RTT" (x 173 to 616), and ends before the next separator (x 647); the SKYBOUND panel and shield bar fill the rest.
Save-Shot (Join-Path $stills 'pc-list.png') 'pc-list.png'
Save-Shot (Join-Path $stills 'library.png') 'library.png'
Save-Shot (Join-Path $stills 'landscape-compact.png') 'stream.png' @(160, 0, 486, 1080)
Save-Shot (Join-Path $stills 'pip.png') 'pip.png'
foreach ($stale in 'overlay.png', 'presets.png') {
    $path = Join-Path $shots $stale
    if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path }
}
if ($SkipGif) { return }

# demo.gif: the tour's strongest motion (the tap from the PC list into the library, the tap on a game into the stream,
# the stream menu opening, the overlay switching to Statistics), cut from the rendered 9:16 video
# and cropped to the phone column (x 120 to 960) with its heading and caption, so the app is large enough to read when
# the GIF is shown 360 px wide on a phone. 30 fps (every other frame of the 60 fps tour) keeps it smooth; the width
# and an 80-colour palette keep it near 2 MB.
function Find-FFmpeg {
    $command = Get-Command ffmpeg.exe -ErrorAction SilentlyContinue
    if ($command) { return $command.Source }
    $binary = Get-ChildItem -Path (Join-Path $env:LOCALAPPDATA 'Microsoft/WinGet/Packages/Gyan.FFmpeg_*/*/bin/ffmpeg.exe') -ErrorAction SilentlyContinue |
        Sort-Object FullName | Select-Object -Last 1
    if ($binary) { return $binary.FullName }
    throw 'FFmpeg is not accessible.'
}
$ffmpeg = Find-FFmpeg
# A fresh render.ps1 run leaves the video and its segment start times in out/demo. Without one, use the published
# site/assets/demo-9x16.mp4 and work the start times out from the shotlist the same way render.ps1 does: a 3-second
# intro, then each shot's seconds in table order.
$video = Join-Path $root 'out/demo/demo-9x16.mp4'
$segmentsJson = Join-Path $root 'out/demo/segments.json'
if ((Test-Path -LiteralPath $video) -and (Test-Path -LiteralPath $segmentsJson)) {
    $segments = (Get-Content -LiteralPath $segmentsJson -Raw | ConvertFrom-Json).'9x16'
} else {
    $video = Join-Path $root 'site/assets/demo-9x16.mp4'
    $segments = @{}; $at = 3.0
    foreach ($line in Get-Content -LiteralPath (Join-Path $root 'docs/demo/shotlist.md')) {
        if ($line -match '^\| (\d{2}-[a-z-]+) \| (\d+) \|') { $segments[$Matches[1]] = $at; $at += [int]$Matches[2] }
    }
}
# Shot ID, seconds into the shot, length: each cut starts just before the shot's action and holds briefly on its result.
$cut = @(@('01-launch', 0.7, 2.0), @('05-stream', 0.8, 2.8), @('15-stream-menu', 0.6, 1.6), @('16-statistics', 0.6, 2.2))
$gifFps = 30
$gifWidth = 360
$chains = @(); $joined = ''
for ($i = 0; $i -lt $cut.Count; $i++) {
    $start = [double]$segments.($cut[$i][0]) + $cut[$i][1]
    $chains += "[s$i]trim=start=$($start.ToString('0.###', [Globalization.CultureInfo]::InvariantCulture)):duration=$($cut[$i][2].ToString('0.###', [Globalization.CultureInfo]::InvariantCulture))," +
        "setpts=PTS-STARTPTS,crop=840:1100:120:340[v$i]"
    $joined += "[v$i]"
}
$splits = '[0:v]split=' + $cut.Count + ((0..($cut.Count - 1) | ForEach-Object { "[s$_]" }) -join '')
$graph = $splits + ';' + ($chains -join ';') + ";${joined}concat=n=$($cut.Count):v=1:a=0,fps=$gifFps[o]"
$graphFile = Join-Path $work 'gif.ffgraph'
[IO.File]::WriteAllText($graphFile, $graph, (New-Object Text.UTF8Encoding($false)))
$gif = Join-Path $assets 'demo.gif'
Push-Location $work
try {
    # Two passes: cut and crop the clips into an intermediate file, then build the palette GIF from it (one combined
    # graph with split, trim and palettegen mixes up the cuts in current FFmpeg).
    $ErrorActionPreference = 'Continue'
    & $ffmpeg -hide_banner -nostdin -y -loglevel error -i $video -/filter_complex gif.ffgraph -map '[o]' -an -c:v libx264 -crf 10 -preset veryfast -pix_fmt yuv420p gif-cut.mp4 2>&1 | Out-Host
    if ($LASTEXITCODE -ne 0) { throw 'FFmpeg could not cut the GIF source.' }
    & $ffmpeg -hide_banner -nostdin -y -loglevel error -i gif-cut.mp4 `
        -vf "scale=${gifWidth}:-1:flags=lanczos,split[a][b];[a]palettegen=stats_mode=diff:max_colors=80[p];[b][p]paletteuse=dither=bayer:bayer_scale=5:diff_mode=rectangle" `
        -loop 0 $gif 2>&1 | Out-Host
    $exit = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    if ($exit -ne 0) { throw 'FFmpeg could not build the GIF.' }
} finally { Pop-Location }
Write-Host ('Saved demo.gif ({0:N1} MB)' -f ((Get-Item -LiteralPath $gif).Length / 1MB))
