# Nightjar site

Plain HTML, CSS and JavaScript. No build step or package installation.
All site navigation and assets use relative paths. External links point to
the release, source documentation and supported streaming hosts.

## Preview and check

From the repository worktree:

```powershell
& C:\nodejs\node.exe site/preview.mjs
```

Open http://127.0.0.1:4317/nightjar/. The project prefix mirrors
GitHub Pages. Stop the server with Ctrl+C. Unknown paths serve the custom
404 page, including nested paths.

In another terminal:

```powershell
& C:\nodejs\node.exe site/check-links.mjs
& C:\nodejs\node.exe --check site/assets/site.js
```

The checker validates local files, anchors, relative paths and linked repository
documents. It checks external URL syntax without making network requests.
It also fails when the assets, excluding video, exceed 6 MB.

## Media

- `assets/shot-library.webp`, `shot-pcs.webp`, `shot-presets.webp` and
  `shot-upscaling.webp`: dark-theme demo-mode captures from `docs/screenshots/demo/`
  (`library.png`, `pc-list.png`, `settings-presets.png`,
  `upscaling-options.png`), resized to 720 px wide. Dark shows the night and
  amber identity and matches the site's dark phone frames in both page themes.
- `assets/shot-stream.webp`: `docs/screenshots/demo/landscape-compact.png`
  cropped to 16:9 (x 150 to 2070, so the compact overlay in the screen's
  top-left corner stays whole) and resized to 1600 × 900.
- `assets/launcher.png` (192 × 192) and `assets/favicon.png` (32 × 32): the
  launcher icon `app/src/main/res/mipmap-anydpi/ic_launcher.xml` (night circle,
  amber moon, bird), rendered as SVG with a transparent background outside the circle.
- `assets/og.png`: the 1200 × 630 raster export of `assets/og.svg`.
- `assets/demo-16x9.mp4` and `assets/demo-9x16.mp4`: the 69 s tour (PCs and
  library, PC settings, stream with the compact overlay, stream menu, Statistics
  with host telemetry, PyroWave and adaptive refresh, upscaling, the Snapdragon GPU
  driver, the DualSense driver and picture-in-picture), H.264 at 60 fps with the
  metadata at the start of the file and no audio track. They are also attached to
  the latest release. `assets/poster-16x9.webp` and `poster-9x16.webp` are the
  posters: a frame of the stream shot, resized from the matching PNG frames.
- Keep everything in `assets/` other than video under 6 MB in total. The link
  checker enforces this.

The demo uses native controls, `preload="none"` and `playsinline`; nothing
autoplays and no video bytes are fetched before play. The markup carries the
16:9 source. `site.js` swaps in a fresh element with the 9:16 source and
poster on screens up to 600 px wide, and only before playback starts.

The privacy page follows `docs/PRIVACY.md`, including public network checks,
local diagnostics, report sharing and Android backups. Endpoint names stay
in the linked source policy. Update both documents together when policy changes.

## Device results

`devices.html` renders `data/device-results.json` with `assets/devices.js`: one
row per measured decoder, sortable by every column and filterable by codec.
The data file starts empty. To add a report posted with the
[device results issue form](../.github/ISSUE_TEMPLATE/device-results.yml), save
the pasted text from the issue to a file and run, from the repository root:

```powershell
& C:\nodejs\node.exe scripts/add-device-results.mjs results.txt --issue 123 --name "Galaxy S23" --dry-run
& C:\nodejs\node.exe scripts/add-device-results.mjs results.txt --issue 123 --name "Galaxy S23"
```

The script accepts format version 1 only, drops unknown fields and refuses
text that looks like an address, e-mail, ID, token, path or markup. Fix or
reject the report instead of editing the data file by hand. Also add the
device to the table in `docs/devices.md`.

## Review

Keep captures in `site/_review/` (ignored by Git). Review each of the eight
HTML pages at 360, 768 and 1440 CSS pixels, in light and dark mode.

- No unintended overlap or horizontal page scrolling.
- No clipped text, broken images, blurry screenshots or unfinished copy.
- Consistent spacing, readable line lengths and visible keyboard focus.
- Navigation, language links, skip links and in-page links work.
- Reduced motion disables smooth scrolling.
- Videos show their poster and expose working controls; narrow screens get the 9:16 tour.

## Publish an orphan gh-pages branch

Run these commands yourself from the worktree after reviewing the site.
They publish only the site, using a separate temporary repository. The new
`gh-pages` branch starts with a root commit and has no Android commit history.
No custom Actions workflow is used.

```powershell
$siteSource = (Resolve-Path ./site).Path
$pagesFolder = Join-Path $env:TEMP ('nightjar-pages-' + [guid]::NewGuid().ToString('N'))
$pagesRemote = 'https://github.com/RamazanKara/nightjar.git'

git init --initial-branch=gh-pages $pagesFolder
Get-ChildItem -LiteralPath $siteSource -Force |
    Where-Object { $_.Name -notin @('_review', '.gitignore', 'README.md', 'preview.mjs', 'check-links.mjs') } |
    Copy-Item -Destination $pagesFolder -Recurse

git -C $pagesFolder add .
git -C $pagesFolder commit -m "docs(site): publish Nightjar site"
git -C $pagesFolder remote add origin $pagesRemote
git -C $pagesFolder push -u origin gh-pages
```

For later updates, clone the existing branch into a fresh temporary directory,
replace its tracked site files with the current `site/` contents using the
same exclusions, then commit and push normally. Keep the existing branch
history; do not create another orphan branch for an update.

Enable branch-based Pages using an authenticated repository administrator
account:

```powershell
gh api --method POST repos/RamazanKara/nightjar/pages -f build_type=legacy -f "source[branch]=gh-pages" -f "source[path]=/"
```

When Pages already exists, update its source:

```powershell
gh api --method PUT repos/RamazanKara/nightjar/pages -f build_type=legacy -f "source[branch]=gh-pages" -f "source[path]=/"
gh api repos/RamazanKara/nightjar/pages --jq .html_url
```

The published site is https://ramazankara.github.io/nightjar/.
`.nojekyll` keeps the published directory static. The Pages API source and
`legacy` build type are documented in the
[GitHub Pages REST API](https://docs.github.com/en/rest/pages/pages#create-a-github-pages-site).
