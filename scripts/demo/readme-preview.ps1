param(
    # Checkout whose README.md and docs/assets are previewed.
    [string]$Docs = ([IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))),
    [string]$Desktop = 'C:\src\readme-new-1440.png',
    [string]$Mobile = 'C:\src\readme-new-393.png'
)

# Renders README.md the way GitHub shows it, at a 1440 px desktop and a 393 px phone viewport, so image legibility can
# be reviewed before pushing. The markdown is rendered by GitHub's own renderer (gh api markdown, GFM mode; read-only,
# nothing is published), wrapped in GitHub-like light-theme CSS and captured with headless Edge. Edge will not make a
# window narrower than about 500 px, so the phone view is a 393 px wide column in a wider window, cropped to 393.
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$edge = @('C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe', 'C:\Program Files\Microsoft\Edge\Application\msedge.exe') |
    Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
if (-not $edge) { throw 'Microsoft Edge is required for the preview.' }
$readme = Join-Path $Docs 'README.md'
# gh prints UTF-8; decode it as such so the middle dots and arrows in the README are not turned into mojibake.
$previousEncoding = [Console]::OutputEncoding
[Console]::OutputEncoding = New-Object Text.UTF8Encoding($false)
try { $body = & gh api markdown -f "text=$([IO.File]::ReadAllText($readme))" -f mode=gfm } finally { [Console]::OutputEncoding = $previousEncoding }
if ($LASTEXITCODE -ne 0) { throw 'gh api markdown failed.' }
$base = ([Uri](Join-Path $Docs '.')).AbsoluteUri
$css = @'
body{margin:0;background:#fff;color:#1f2328;font:16px/1.5 -apple-system,"Segoe UI","Noto Sans",Helvetica,Arial,sans-serif}
.page{max-width:1012px;margin:24px auto;border:1px solid #d1d9e0;border-radius:6px}
.head{padding:10px 16px;background:#f6f8fa;border-bottom:1px solid #d1d9e0;font-size:14px;font-weight:600;border-radius:6px 6px 0 0}
article{padding:32px;word-wrap:break-word}
[align=center]{text-align:center}
img{max-width:100%;box-sizing:content-box;background-color:#fff}
a{color:#0969da;text-decoration:none}
h2{font-size:1.5em;font-weight:600;margin:24px 0 16px;padding-bottom:.3em;border-bottom:1px solid #d1d9e0b3;line-height:1.25}
p,ul,ol{margin-top:0;margin-bottom:16px}
ul,ol{padding-left:2em}
li+li{margin-top:.25em}
.anchor{display:none}
'@
$phoneCss = 'html{width:393px;overflow-x:hidden}.page{margin:0;border-radius:0;border-left:0;border-right:0}.head{border-radius:0}article{padding:16px}'

function Capture([int]$Width, [string]$Path, [bool]$Phone) {
    $extra = $(if ($Phone) { $phoneCss } else { '' })
    $html = "<!doctype html><html><head><meta charset='utf-8'><base href='$base'><style>$css$extra</style></head>" +
        "<body><div class='page'><div class='head'>README.md</div><article>$($body -join "`n")</article></div></body></html>"
    $page = Join-Path ([IO.Path]::GetTempPath()) "nightjar-readme-preview-$Width.html"
    [IO.File]::WriteAllText($page, $html, (New-Object Text.UTF8Encoding($false)))
    $raw = "$Path.raw.png"
    $window = [Math]::Max($Width, 600)
    & $edge --headless=new --disable-gpu --hide-scrollbars --force-device-scale-factor=1 --window-size="$window,4200" `
        --virtual-time-budget=4000 --screenshot="$raw" ([Uri]$page).AbsoluteUri 2>&1 | Out-Null
    if (-not (Test-Path -LiteralPath $raw)) { throw "Edge produced no screenshot at ${Width}px." }
    # Crop to the requested width and trim the empty page background below the card.
    $bitmap = [Drawing.Bitmap]::FromFile($raw)
    try {
        $last = $bitmap.Height - 1
        $background = $bitmap.GetPixel($window - 3, $bitmap.Height - 2)
        while ($last -gt 0) {
            $differs = $false
            for ($x = 0; $x -lt $Width; $x += 7) {
                if ($bitmap.GetPixel($x, $last).ToArgb() -ne $background.ToArgb()) { $differs = $true; break }
            }
            if ($differs) { break }
            $last--
        }
        $crop = New-Object Drawing.Rectangle(0, 0, $Width, [Math]::Min($bitmap.Height, $last + 26))
        $trimmed = $bitmap.Clone($crop, $bitmap.PixelFormat)
        try { $trimmed.Save($Path, [Drawing.Imaging.ImageFormat]::Png) } finally { $trimmed.Dispose() }
    } finally { $bitmap.Dispose(); Remove-Item -LiteralPath $raw }
    Write-Host "Saved $Path"
}
Capture 1440 $Desktop $false
Capture 393 $Mobile $true
