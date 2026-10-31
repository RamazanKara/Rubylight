param(
    # Repository root whose docs/assets receives the README images (defaults to this checkout).
    [string]$Docs = ([IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..')))
)

# Builds the README's phone screenshots (docs/assets/shots) from the stills in docs/screenshots/demo. The README's
# animated teaser is docs/assets/nightjar-trailer.webp from trailer.py.
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
$stills = Join-Path $root 'docs/screenshots/demo'
$assets = Join-Path $Docs 'docs/assets'
$shots = Join-Path $assets 'shots'
$null = New-Item -ItemType Directory -Force $shots
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
