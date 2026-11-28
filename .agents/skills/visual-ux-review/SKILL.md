---
name: visual-ux-review
description: >-
  Use whenever changing or reviewing user-facing Rubylight or Nightjar UI
  (Console, /play, /guest, Couch Menu, Installer, Tray, Nightjar Phone/Handheld/TV/Desktop,
  Websites, READMEs) to render and visually inspect screenshots across themes,
  viewports, and German/CJK stress using Read.
---

# Visual UX & Accessibility Review Protocol

Never approve a visual, UI, demo, site, or README change from code inspection alone — render screenshots and inspect EVERY image directly with `Read`.

## 1. Capture Matrix (Every User-Facing Surface)

### A. Rubylight 7.0 Surfaces (5 Surfaces)
1. **Rubylight 7.0 Web Console (`rust/web/`)**:
   - `node --test Tokens.test.mjs Components.test.mjs` (enforcing >= 4.5:1 text / >= 3.0:1 focus contrast on the new 7.0 token system).
   - `node scripts/screenshots.cjs <out-dir> <label>` + `Design.browser.cjs` across Light + Dark, Desktop (`1440 px`) + Phone (`390 px`), covering Live-Stream Command Stage, Display Topology Visualizer, Cover-Lit Per-Game Studio, QR Onboarding, `Ctrl+K` Command Palette, Settings, `/design.html`, and scriptless fallback (`rust/host/web/src/console/`).
2. **Browser `/play` & `/guest` Web Apps**:
   - Playwright fixture captures of `/play` (active stream dock + stats HUD) and `/guest` (join card + active guest controls) at `1440 px` and `390 px` in Dark + Light.
3. **Host-Drawn Couch Menu (L3+R3 overlay for stock Moonlight clients)**:
   - Deterministic software/CPU rasterized frame captures (`rust/core/session/src/couch_menu_model/` + `rust/host/stream/src/couch_menu/`) at `1280×720` and `1920×1080` showing crisp console cards, typography, active focus ring, and readable sliders.
4. **Windows Installer (`rust/setup/`) & System Tray (`rust/windows/io/src/tray/`)**:
   - Headless UI / screenshot renders of installer welcome, progress, `rubylight-vgamepad` status, instant QR pairing finish screen, and system tray status menu in Light + Dark.

### B. Nightjar 7.0 Surfaces (All Form Factors)
- **Design Prototype (`client/design/prototype/`)**:
  ```powershell
  node tokens-css.mjs
  node check-tokens.mjs
  node shoot.mjs
  python check-contrast.py
  ```
  Cover Night, Day (`?theme=day`), OLED (`?theme=oled`), High Contrast (`?hc=1`), Reduced Motion (`?motion=reduce`), German stress (`?stress=1&lang=de`), Remote vs Gamepad (`?input=remote|pad`), and `lens|live|baked` tiers across Console Home, Console Control Center floating dock, Visual Preview-Card Settings, and First-Run/Pairing Cinema.
- **Desktop (`client/protocol/desktop-ui/src/shot.rs` + `ui/tests.rs`)**: `1280×800` Desktop, `1280×720` Handheld, 4K scaling (`#144`/`#146`), Night/Day/OLED/HC, German & Japanese.
- **Android (`client/scripts/demo/capture-stills.ps1`, `client/scripts/emulator-smoke.py`)**: Phone (`393dp-font-1`, `412dp-font-1`, `393dp-font-1.3`, `393dp-font-2`, `393dp-font-2-landscape`, Reverse Landscape `#152` + Safe Area / manual centering `#190`), Handheld, Android TV (`1920×1080`), Modern Virtual Controller Studio (`#151`), and Console Control Center open/close blur verification (`#61`).

### C. Websites & READMEs
- **Rubylight site (`site/`)**: `python site/build.py` -> inspect rendered `_site/` pages at **360, 768, 1440 px** in **Light and Dark** mode.
- **Nightjar site (`client/site/` + `de/`)**: `node client/site/preview.mjs` + `node client/site/check-links.mjs` -> inspect all 8 HTML pages at **360, 768, 1440 px** in **Light and Dark** mode, asserting `< 6 MB` non-video assets.
- **READMEs**: `client/scripts/demo/readme-preview.ps1` (1440 px and 393 px GitHub markdown render).

## 2. Multimodal Inspection Checklist (via `Read`)

Open each rendered PNG/JPG/WebP with `Read` and fail the review if ANY of the following occur:

1. **Console-Quality Bar**: Does it look like a flagship cohesive 7.0 console OS (PS5 × Apple tvOS × SteamOS OLED × studio hardware), with strong typography, balanced negative space, and zero legacy/unpolished widgets?
2. **Contrast & Readability**: text < 4.5:1 or focus/outline < 3.0:1; washed-out text over worst-case white (night) or black (day) cover art; unreadable status badges.
3. **Layout, Scaling & Cutouts**: horizontal scroll at 360/390 px; broken resolution-aware scaling (#144/#146); clipped headings/values; overlapping chrome, camera cutouts (#152/#190), or floating docks hiding the final list row.
4. **German, CJK & Large-Font Stress**: truncated values or single-word broken buttons under `?lang=de&stress=1`, CJK (`ja`/`zh`), or 2.0× font scale.
5. **Hit Targets, Focus & Input Glyphs**: clickable controls < 44×44 px (console) or < 48×48 dp (Nightjar; 1.5× TV); missing focus ring; stale/wrong Xbox/PS/Nintendo/Keyboard/TV-Remote glyphs; any hint shown for Touch/Mouse.
6. **Motion & Stream Safety**: nested blur layers; compositor flicker (#61); ANY blur/shadow/extra layer active over video when the Console Control Center is closed; non-reduced-motion pulses.
7. **Copy & Honest Assets**: any `"Experimental"` text, any mod name (Nucleus Co-op / ANR), any uncredited/third-party game art (original covers only), or synthetic demo stats presented without a DEMO caption.
