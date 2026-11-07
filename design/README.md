# Nightjar design: moonlit glass

The spec for the redesign. Numbers live in [tokens.json](tokens.json), the single source (flat dotted keys). `python scripts/gen_design_tokens.py` from `client/` emits Android XML, Rust constants and CSS; `python scripts/test_gen_design_tokens.py` fails if they are stale. This file says how to build them.

The prototype is in [prototype/](prototype/) (`index.html` lists every screen; `?theme=day|oled`, `?tier=lens|live|baked`, `?hc=1`, `?motion=reduce`, `?stress=1`, `?lang=de`, on TV `?input=remote|pad`). To run it, from `client/design/prototype`:

```
npm install
python ambient.py                    # bakes ambient/, night and day
node tokens-css.mjs                  # tokens.css from tokens.json
node check-tokens.mjs                # fails on any contrast or spring miss, writes nothing
node shoot.mjs [name...]             # shots/
python check-contrast.py             # measures the worst pixel in the shots
```

`covers/` and `frames/` are not in the repository, so a fresh checkout shows empty cover slots. To see art, point `coverSrc` in `lib.js` at the fixtures in `app/src/debug/assets/demo/art/`.

`shots/`, `covers/`, `frames/`, `ambient/`, `tokens.css` and `node_modules/` are generated and ignored. Figtree comes from `client/protocol/desktop/assets`.

## Principles

1. **Amber is the only thing lit from inside.** The moon Play capsule is solid, never glass. Selection is a dark moon container (`tint.selected.fill`, text `color.onSelected`), focus is a moon ring. Nothing else in the app is amber.
2. **Art goes edge to edge, chrome floats over it.** Glass is for chrome only: tab bar, capsules, sheet, dialog, menu. Art-less pages (PCs, Settings, Pair) get the night sky, moon and orbit rings.
3. **No nested blur.** One blurred layer per surface. Groups, controls and selections inside glass are flat tints (`tint.*`), and never blur.
4. **Text reads over the worst cover in every tier.** A white cover at night, a black cover by day. 4.5:1 is a property of the recipe, not of the art: see below.
5. **Simple first.** No instruction sentences, no toggle for what is always right, one hover and focus language. The word "Experimental" appears nowhere.

## Glass recipe

Layers, back to front: backdrop → blur → saturate → **brightness** → fill → lens band → sheen (vertical gradient `sheenTop` to clear at `sheenEnd`) → rim (`rimTop`, `rimMid`, `rimBottom`) → shadow behind all of it. Brightness before fill is what keeps white backdrops legible: worst case is `white × brightness × (1 − fillAlpha) + fill`.

Materials: `chrome` (tab bar, capsules), `panel` (sheet, dropdown, dialog), `menu` (stream menu), `chip` (labels on art). Keys are `glass.<material>.<tier>.<property>`.

| Tier | What it is | Android | Desktop (egui over SDL) | Web |
|---|---|---|---|---|
| `lens` | Live blur plus a refracted edge band | API 33+: `RenderNode` + `RenderEffect` blur chained with a colour matrix, `RuntimeShader` for the band | Ring of quads with displaced UVs over the live chain, no shader | `backdrop-filter` plus a masked second layer |
| `live` | Live blur, no band | API 31-32 or a lens probe failure | Linear down/upsample chain through SDL render targets, sampled with screen-space UVs | `backdrop-filter` |
| `baked` | Samples the static ambient in screen space, × brightness, under the fill. Live content behind does not show | API 21-30, low RAM, failed probe, power saver | CPU-blurred backdrop texture, same UV meshes | Not needed |
| `solid` | High contrast: opaque fill, 1.5 px light rim, no sheen or shadow | Same | Same | `@media (prefers-contrast: more)`, `prefers-reduced-transparency` |

**Landing on a tier.** By capability, never by device name. Android: API level, then `ActivityManager.isLowRamDevice`, `PowerManager.isPowerSaveMode`, thermal status (API 29+), then a `tier.probe.frames` frame probe with `FrameMetrics`: p90 above `tier.probe.p90BudgetFraction` of the frame budget steps down one tier. Persist the result per app version. Desktop: the render-target chain timed the same way over the first frames. A cheap TV on `baked` is a full design, not a degraded one: check `variant-tv-home-baked.png`.

**Android live blur.** Draw the content into a `RenderNode` at 1/4 scale, apply `RenderEffect.createBlurEffect` chained with `createColorFilterEffect` (saturation and brightness in one matrix), draw it clipped to the glass shape, then fill, sheen and rim. One shared backdrop node per window; each glass surface samples it with its own clip. The band shader only brightens and shifts UVs in the outer `lensWidth` dp.

**Desktop.** `gui.rs` walks `Primitive::Mesh` and skips the rest: add a `Shape::Callback` marker as the boundary between content and chrome. Paint content, snapshot the regions under chrome, run the chain, then draw chrome meshes textured with screen-space UVs × vertex colour (brightness is a multiply; saturation exists only at bake time, so `live` on desktop keeps the bake's colour lift). Rim is a ring mesh with a vertex-colour gradient and a feather ring for antialiasing. Shadows and the focus halo are baked nine-slices. Corners are circular.

**Stream menu.** While a stream plays with no menu open there is **no blur, no extra layer, nothing added**: a hard rule. The stats capsule is a flat fill (`stats.fill`), one quad in the existing overlay. Opening the menu may add one window:
- Android API 31+ with `WindowManager.isCrossWindowBlurEnabled`: a separate window with `setBackgroundBlurRadius`, only behind the panel. This blur has no colour filter, so it uses `glass.menu.window.*` (`variant-tv-stream-menu-window.png`). Listen for the blur-enabled callback; battery saver or multimedia tunneling can turn it off at any time.
- Otherwise `glass.menu.baked.*`, a near-opaque fill (`variant-tv-stream-menu-fallback.png`).
- Desktop: the chain runs from the video texture only while the menu is open.

The window is removed (not hidden) when the menu closes, and the frame after closing must match the frame before opening in composition cost.

### Ambient backdrop

Baked once per cover, never per frame. Reference: `prototype/ambient.py`. Squash the **whole** cover to 96×54 (or 54×96 in portrait; hosts give 2:3 box art, no landscape art is assumed anywhere), Gaussian blur, saturate × `ambient.saturate`, hue-shift yellows (35-80°) toward 22° so they never dim to olive, then clamp each pixel's linear luminance to `ambient.textZoneMaxLuma`. A white cover comes out as a safe grey (82). Add ±1 LSB noise at bake: 96×54 stretched over a large dark panel can band.

This replaces the current bake in `ui/BackdropBlur.java`. `BackdropBlurTest` changes with it: `A9ADBD` leaves the text list, and `aWhiteCoverStillShowsThroughBelowTheTextZone` goes away because a white cover is now a uniform clamp grey. Assert the clamp and the hue shift instead.

## Themes

Android keeps its theme setting: System (default), Dark, OLED black (`preferences/ThemeChoice.java`, `ui/AppTheme.java`, `res/values-notnight/colors.xml`). System follows the phone, so a phone in light mode gets **day**. Desktop is night only. Every themed key has an override with the same name under `day.*` or `oled.*`; a missing override means the night value.

| Theme | Canvas | Glass | Worst backdrop | Picks |
|---|---|---|---|---|
| Night | `color.canvas`, night sky | `glass.*` | White cover | Dark, or System at night |
| Day, "morning light" | `day.color.canvas`, dawn sky (`day.sky.*`), the moon becomes a low sun, no stars | `day.glass.*` | Black cover | System by day. Android only |
| OLED | True black (`oled.color.*`, `oled.sky.*`), ambient fades to `oled.tint.fade` | `glass.*`, unchanged | White cover | OLED black. Android only |

**Day glass.** Milky, lit from above, not an inverted night. The recipe order stays; the numbers change: brightness `day.glass.*.brightness` above 1, a stronger saturate so colour blooms through, white fill (`day.glass.<material>.<tier>.fill`, about 70% on chrome and chip, 78% on panel), white rim, a cool shadow, and one `hairline` so glass keeps its edge over white. Ink text (`day.color.label`, `day.color.labelSecondary`). Worst case `black` still gives 4.5:1 because the day ambient lifts it first.

**Day ambient.** Same squash and blur, then saturate × `day.ambient.saturate` and mix each pixel toward white until its luminance reaches `day.ambient.textZoneMinLuma`. Black bakes to a soft grey (188), never to dark. `ambient.py` writes both.

**Day accents.** Play stays the solid moon. Selection is a light amber (`day.tint.selected.fill`, text `day.color.onSelected`). Focus is a dark amber ring on a white gap with an amber halo; on Play the ring is ink. Neighbours on TV fade with `day.focus.neighbourAlpha` instead of dimming, because dimming muddies a light page. The page behind a sheet barely dims (`day.sheet.backdropBrightness`).

**What does not change with theme.** Covers, the no-art tile and the Play gradient. The stream menu and stats stay night in every theme: a stream is usually dark and a white panel over it would glare. There is no `day.glass.menu`. High contrast is always black and wins over day, as `AppTheme.isOledBlack` does today.

## Navigation

**Two ways in.** A host with the native library API (Rubylight) lands on Home. Every other host (Sunshine, Apollo and the rest) lands on the PC list and then the app grid, and a new user always starts on an empty PC list. These are full screens, as finished as Home. Home's top capsule and the app grid's use the same PC capsule, so the two paths feel like one app.

| Form factor | Top level | Game page | Settings |
|---|---|---|---|
| Phone | Floating bottom tab bar (Home, PCs, Settings) plus a round search button; PC capsule at the top of Home | Sheet with detents `sheet.detentMedium` and `sheet.detentLarge`, cover morphs from its card, page behind scales to `sheet.backdropScale` | Large title, category chips, grouped rows |
| Handheld | Top capsule with the same three tabs, PC capsule on the right; hero with Continue beside it, shelves below | Full page | Side rail |
| TV | Top capsule, left/right along shelves, up to chrome; ambient follows focus | Full page, hero plus shelves, no nav bar; Back returns to the same focused cover | Side rail, one column |
| Desktop | Top capsule, keyboard and controller walk everything | Full page | Sidebar, dropdowns anchored to the row |

One rule everywhere: Back closes the top layer and restores focus to what opened it. Nothing is reachable by touch or mouse only, and nothing by remote only. Below `layout.settingsMaxWidth` everything is one column.

## Components

All have default, hover, focus, pressed and disabled, on `solid` too. Touch targets are at least `size.touchMin` (48 dp); TV sizes are the same dp at 1.5×.

- **Tab bar**: `chrome`, height `size.tabBarHeight`. The selected pill springs (`motion.tabPill`) and stretches slightly toward the target. It travels as one layer so it never redraws the tabs' text. Under the bar, one 136 dp gradient to `tint.fade` (no blur, no extra surface) so names and art never sit half hidden at rest. Scrolling lists also end with bottom padding of bar height plus `space` so the last row clears it.
- **Top capsule**: `chrome`, `size.topCapsuleHeight`. Status dot, PC name, chevron. Same pill as the tab bar.
- **Cover card**: 2:3 fixed (`cover.aspect`), `radius.cover`, rim and top sheen, shadow. Status dot on a dark disc at the bottom right for the running game, clear of the logotype. Label below on phone, only on focus on TV. In the app grid there is no label: art carries its title and so does the no-art tile. The accessibility label stays.
- **No-art tile**: see below. Same slot, rim, sheen and shadow as a cover.
- **PC card**: a small screen on a stand (rounded rect, neck, base; a few filled shapes at run time), then name, status and one action. Flat `tint.groupOnSky`, never glass: the sky behind it has nothing to refract, and glass over it only reads as a dark box. See "PC states".
- **Button hint**: a key cap plus the word, following the input device. See "Button hints".
- **Play button**: solid `play.*` gradient, moon rim, soft glow, dark text. One per screen. Says Play, or Resume when running.
- **Sheet**: `panel`, `radius.sheet`, grabber, drag between detents, flick to close. Scroll hands over to drag at the top edge.
- **Grouped row**: `tint.group` (`tint.groupOnArt` on art), `size.rowHeight`, leading icon optional, value and chevron trailing. Separators inset past the icon. Long German labels wrap to two lines, values never truncate first.
- **Switch, slider, segmented, chips**: tints only. Moon for on and filled, paper thumb. The slider shows its value at all times and steps with D-pad left and right.
- **Dialog**: `panel`, centred, one primary action in moon; destructive uses `color.dangerFill`.
- **Toast**: `chip`, top on phone and above the shelves on TV, one line, no action needed to dismiss.
- **Stream menu**: `menu`, right side, 452 × 672 at 720p scale. Tile row, sliders, status rows, Disconnect and Quit game at the bottom. Quit game is the one red control.
- **Stats overlay**: `stats.fill`, tabular numbers (`type.numbers`), label colour only. Never blurred.
- **PIN pad**: `panel` sheet on phone, dialog elsewhere. Four digit slots, the next one focused, a 3×4 grid of `tint.control` capsules with scan-a-code at bottom left and delete at bottom right. The remote's digits and D-pad both work.

**One word.** Ending a game is "Quit game" everywhere: sheet, TV page, desktop, stream menu, PC list (`applist_menu_quit`, German "Spiel beenden"). Never "Close game".

Cover style: "night posters", drawn by code, fixtures only. Generator `client/scripts/demo/covers.mjs`: layered silhouettes, gradient skies, one glow, grain, vignette, a logotype per game in Figtree only. One cover per demo game, committed and regenerated by hand (`client/docs/demo/README.md`). No sprite, costume or logo from a real game or character. This generator does not ship in the app.

### No-art tile

What an app with no cover art shows, on Android (`Canvas`) and in egui (a gradient mesh, a circle, text). No browser, no procedural art, no decode, nothing cached. It must look like a decision, not a gap. Same in every theme.

1. `h` = FNV-1a 32-bit over the title's lowercase UTF-8 bytes (offset `0x811C9DC5`, prime `0x01000193`, unsigned).
2. Background: linear gradient at 165° from `tile.pair<N>.top` to `tile.pair<N>.bottom`, `N = h % 12`. All 12 pairs pass 4.5:1 with white text at their lightest point; `check-tokens.mjs` enforces it.
3. Disc: one filled circle, white at `tile.discAlpha`, diameter `tile.discScale` × width, centre from `(h >>> 8) % 4` into `[.74,.62] [.26,.7] [.8,.42] [.5,.78]` (x × width, y × height).
4. Title: white, Figtree 600, top left, inset `tile.insetScale` × width, size `tile.titleScale` × width, line height 1.08, tracking -0.01 em, up to 3 lines, then an ellipsis. If the longest word does not fit, drop once to `tile.titleMinScale` × width. Measure at run time; the prototype only estimates.
5. Then the cover's own sheen, rim, radius and shadow.

As a backdrop (TV app grid, connecting), a no-art app uses its gradient where a cover would use its ambient. Two titles can share a pair: 12 pairs and 4 disc positions make 48 looks, and the title tells them apart. Reference: `tile()` in `prototype/lib.js`.

## Focus and hover

One language. **Focus**: `focus.ringColor` ring, dark gap, halo, small lift (`focus.scale*`), neighbours dim on TV (`focus.neighbourDim`). On the moon Play button the ring is paper (`focus.ringColorOnMoon`). **Hover** is the same step quieter: rim brightens, fill lifts, no ring. It has to read from across a room on a `baked` TV.

### Button hints

Hints show what the device in the hand really has. Take the source from the last `InputEvent` and its `InputDevice`; on desktop, the last SDL event. Never assume from form factor. Change on the next input, with no animation.

| Last input | Go | More |
|---|---|---|
| Remote (D-pad, no gamepad buttons) | OK | Hold OK |
| Gamepad | A, or the label that pad has | Y, likewise |
| Keyboard | Enter | F |
| Touch, mouse | No hints | No hints |

A remote has no Y, so anything on Y must also open from Hold OK. `prototype/tv-home.html?input=remote|pad`, `variant-tv-home-pad.png`.

## States

- **Loading**: covers keep their 2:3 slot with `cover.placeholder`, art fades in over `motion.coverIn.ms`, no layout jump, no spinner on Home. Ambient cross-fades over `motion.ambient.ms` and follows focus only once it rests.
- **First run**: no PCs. Sky, moon with one orbit dot moving while discovery runs, "Searching for PCs…" (`searching_pc`), one moon action, Add PC. No paragraph. A found PC rises in as a card with Pair as the one action, and the moon button steps down so there is still only one (`phone-first-run.png`, `phone-first-run-found.png`).
- **PC states**: the PC glyph, the word and the action all say the same thing.

  | State | Screen face | Line | Action |
  |---|---|---|---|
  | Running | The game's art | Green dot, game name | Resume, More |
  | Online | Hash gradient plus disc (the tile's recipe, on the PC name) | Green dot, "Online, 12 ms" | Open, or tap the row |
  | Offline | Dark with a crescent (`color.offline`) | Grey dot, "Offline" | Wake (`pcview_menu_send_wol`), when the PC has a saved MAC; its covers stay visible |
  | Unpaired | Hash gradient with a lock | Lock, "Not paired" | Pair (`pcview_menu_pair_pc`), in moon-coloured text, no second moon |

- **App grid**: 3 columns on phone, 7 on TV and desktop. Fixed slots, covers and no-art tiles side by side. A running app gets the status dot and a capsule above the tab bar with Resume. A PC with no apps shows one line and the Desktop tile.
- **Connecting and launching**: one screen. The cover (or tile) sits in the middle of its ambient with the moon's orbit around it and one word: "Connecting" (`conn_establishing_title`), then "Starting" (`conn_starting`). The word changes when the protocol stage does; there is no fake progress bar. The orbit stops on the first decoded frame, with nothing held back for it. Cancel, or Back, stops it (`phone-connecting.png`, `phone-launching.png`).
- **Error**: loud, in words with an icon, in the place the action happened. Never a silent fallback. A failed connection is a sheet over the page it came from, not a new page: "Connection Error" (`conn_error_title`), the app's real message (`early_termination_error` and friends) unshortened, a moon Try again, and Test the network as the second action (`phone-error.png`).
- **New strings** for the builder: "Try again", a short "Wake" (the existing `pcview_menu_send_wol` says "Wake PC", too long for a row), "Test the network". All three need translations. Everything else above already exists in `res/values/strings.xml`.

## Motion

Springs (mass 1), `motion.<name>.stiffness` and `.ratio`, all settle under 250 ms: press 103, focus 180, nav 184, switch 202, tabPill 206, sheet 222. Android: `SpringAnimation` (`androidx.dynamicanimation`, declare it in `app/build.gradle`), retarget on every new input. Desktop: a small damped integrator per value, stepping on real frame time. CSS approximates with beziers, so judge feel on devices, not in the prototype.

- Everything is interruptible: a new input retargets from the current position and velocity.
- Only animate transform and alpha; backdrops update on scroll, never on a timer.
- Launching a game has no exit animation that delays it: the stream starts on the press, the page fades underneath.

**Reduced motion** (`reduced.*`, existing `Motion.reduced()`): springs become short fades, no scale, no overshoot, ambient cuts. **High contrast** (existing overlay): `solid` tier, `hc.color.*`, ring grows (`hc.focus.*`), no halo, covers keep their art with a light rim. See `variant-*-hc.png`.

## Speed budgets

| What | Budget |
|---|---|
| Feedback for any input | Next frame (`motion.firstFeedbackFrames`) |
| Navigation | Interruptible, ≤ `motion.navMaxMs` (250 ms); input during it is honoured at once |
| Main thread | Nothing blocks input: ambient bake and cover decode off-thread, no blur work on first draw |
| Covers | Appear with no layout jump |
| Launch | No animation delays it |
| Stream with no menu | Zero added layers or GPU work compared with today |
| Scroll on `live` and `lens` | Holds the display rate on the probed device, or the tier steps down. The budget floors at 120 Hz: a faster screen keeps live glass while the chain fits a 120 Hz frame |

## Type

Figtree 400 and 600 only, `type.*`. On Android bundle `Figtree-400.ttf` and `Figtree-600.ttf` from `client/protocol/desktop/assets` (49 KB) in `res/font`; below API 26 load via `ResourcesCompat`.

These files have Latin-1 only. pl, tr, vi, ru, uk, ja, ko, zh-CN and zh-TW miss letters (measured against `values-*/strings.xml`). In those languages use the system face for the whole UI, as the desktop's `theme.rs` already does, so a word never mixes two fonts. Tabs and capsules fit German today; keep measuring the longest language per component.

## Public images

`prototype/banner.html` is the README banner. All store and README shots come from fixtures. Public text never claims first, only, fastest or most beautiful.

## Not exact, on purpose

- True refraction exists only on API 33+ (band shader); desktop uses displaced UVs, everywhere else there is no band.
- Desktop has no live saturation and no superellipse corners.
- Cross-window blur has no colour filter, and devices or battery saver can refuse it.
- Day has only been judged on screenshots so far, not on a real phone in daylight or a real TV panel. Judge `day.sheet.backdropBrightness` and the TV sun on hardware.
