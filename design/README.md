# Nightjar design: moonlit glass

The spec for the redesign. Numbers live in [tokens.json](tokens.json) (flat dotted keys; a generator emits Android XML, Rust constants and CSS). This file says how to build them. The prototype is in `drafts/nightjar-glass/prototype/` (`index.html` lists every screen; `?tier=lens|live|baked`, `?hc=1`, `?motion=reduce`, `?stress=1`, `?lang=de`). `drafts/nightjar-glass/mk-tokens.mjs` writes the tokens and fails on any contrast or spring miss.

## Principles

1. **Amber is the only thing lit from inside.** The moon Play capsule is solid, never glass. Selection is a dark moon container (`tint.selected.fill`, text `color.onSelected`), focus is a moon ring. Nothing else in the app is amber.
2. **Art goes edge to edge, chrome floats over it.** Glass is for chrome only: tab bar, capsules, sheet, dialog, menu. Art-less pages (PCs, Settings, Pair) get the night sky, moon and orbit rings.
3. **No nested blur.** One blurred layer per surface. Groups, controls and selections inside glass are flat tints (`tint.*`), and never blur.
4. **Text reads over a white cover in every tier.** 4.5:1 is a property of the recipe, not of the art: see below.
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

Baked once per cover, never per frame. Reference: `drafts/nightjar-glass/ambient.py`. Squash the **whole** cover to 96×54 (or 54×96 in portrait; hosts give 2:3 box art, no landscape art is assumed anywhere), Gaussian blur, saturate × `ambient.saturate`, hue-shift yellows (35-80°) toward 22° so they never dim to olive, then clamp each pixel's linear luminance to `ambient.textZoneMaxLuma`. A white cover comes out as a safe grey (82). Add ±1 LSB noise at bake: 96×54 stretched over a large dark panel can band.

This replaces the current bake in `ui/BackdropBlur.java`. `BackdropBlurTest` changes with it: `A9ADBD` leaves the text list, and `aWhiteCoverStillShowsThroughBelowTheTextZone` goes away because a white cover is now a uniform clamp grey. Assert the clamp and the hue shift instead.

## Navigation

| Form factor | Top level | Game page | Settings |
|---|---|---|---|
| Phone | Floating bottom tab bar (Home, PCs, Settings) plus a round search button; PC capsule at the top of Home | Sheet with detents `sheet.detentMedium` and `sheet.detentLarge`, cover morphs from its card, page behind scales to `sheet.backdropScale` | Large title, category chips, grouped rows |
| Handheld | Top capsule with the same three tabs, PC capsule on the right; hero with Continue beside it, shelves below | Full page | Side rail |
| TV | Top capsule, left/right along shelves, up to chrome; ambient follows focus | Full page, hero plus shelves, no nav bar; Back returns to the same focused cover | Side rail, one column |
| Desktop | Top capsule, keyboard and controller walk everything | Full page | Sidebar, dropdowns anchored to the row |

One rule everywhere: Back closes the top layer and restores focus to what opened it. Nothing is reachable by touch or mouse only, and nothing by remote only. Below `layout.settingsMaxWidth` everything is one column.

## Components

All have default, hover, focus, pressed and disabled, on `solid` too. Touch targets are at least `size.touchMin` (48 dp); TV sizes are the same dp at 1.5×.

- **Tab bar**: `chrome`, height `size.tabBarHeight`. The selected pill springs (`motion.tabPill`) and stretches slightly toward the target. It travels as one layer so it never redraws the tabs' text.
- **Top capsule**: `chrome`, `size.topCapsuleHeight`. Status dot, PC name, chevron. Same pill as the tab bar.
- **Cover card**: 2:3 fixed (`cover.aspect`), `radius.cover`, rim and top sheen, shadow. Status dot for the running game. Label below on phone, only on focus on TV.
- **Play button**: solid `play.*` gradient, moon rim, soft glow, dark text. One per screen. Says Play, or Resume when running.
- **Sheet**: `panel`, `radius.sheet`, grabber, drag between detents, flick to close. Scroll hands over to drag at the top edge.
- **Grouped row**: `tint.group` (`tint.groupOnArt` on art), `size.rowHeight`, leading icon optional, value and chevron trailing. Separators inset past the icon. Long German labels wrap to two lines, values never truncate first.
- **Switch, slider, segmented, chips**: tints only. Moon for on and filled, paper thumb. The slider shows its value at all times and steps with D-pad left and right.
- **Dialog**: `panel`, centred, one primary action in moon; destructive uses `color.dangerFill`.
- **Toast**: `chip`, top on phone and above the shelves on TV, one line, no action needed to dismiss.
- **Stream menu**: `menu`, right side, 452 × 672 at 720p scale. Tile row, sliders, status rows, Disconnect and Quit at the bottom. Quit is the one red control.
- **Stats overlay**: `stats.fill`, tabular numbers (`type.numbers`), label colour only. Never blurred.
- **PIN pad**: `panel` sheet on phone, dialog elsewhere. Four digit slots, the next one focused, a 3×4 grid of `tint.control` capsules with scan-a-code at bottom left and delete at bottom right. The remote's digits and D-pad both work.

Cover style: "night posters", drawn by code. Reference `drafts/nightjar-glass/covers.mjs`: layered silhouettes, gradient skies, one glow, grain, vignette, a logotype per game in Figtree only. Every primitive maps to Java2D, so `client/scripts/demo/GenerateAssets.java` can port them. Fixtures only (Racing Game, Ocean Explorer, Open World, Music Studio, Living-room PC). The same generator, seeded from a name hash, is the in-app cover for games with no art.

## Focus and hover

One language. **Focus**: `focus.ringColor` ring, dark gap, halo, small lift (`focus.scale*`), neighbours dim on TV (`focus.neighbourDim`). On the moon Play button the ring is paper (`focus.ringColorOnMoon`). **Hover** is the same step quieter: rim brightens, fill lifts, no ring. It has to read from across a room on a `baked` TV.

## States

- **Loading**: covers keep their 2:3 slot with `cover.placeholder`, art fades in over `motion.coverIn.ms`, no layout jump, no spinner on Home. Ambient cross-fades over `motion.ambient.ms` and follows focus only once it rests.
- **Empty**: no PCs shows the night sky with the moon and a single action, not a paragraph. A PC with no games shows one line and the desktop tile.
- **Error**: loud, in words with an icon, in the place the action happened. Never a silent fallback.
- **Offline PC**: stays listed with Wake if the host allows it, and its covers stay visible.

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
| Scroll on `live` and `lens` | Holds the display rate on the probed device, or the tier steps down |

## Type

Figtree 400 and 600 only, `type.*`. On Android bundle `Figtree-400.ttf` and `Figtree-600.ttf` from `client/protocol/desktop/assets` (49 KB) in `res/font`; below API 26 load via `ResourcesCompat`.

These files have Latin-1 only. pl, tr, vi, ru, uk, ja, ko, zh-CN and zh-TW miss letters (measured against `values-*/strings.xml`). In those languages use the system face for the whole UI, as the desktop's `theme.rs` already does, so a word never mixes two fonts. Tabs and capsules fit German today; keep measuring the longest language per component.

## Public images

`drafts/nightjar-glass/prototype/banner.html` is the README banner. All store and README shots come from fixtures. Public text never claims first, only, fastest or most beautiful.

## Not exact, on purpose

- True refraction exists only on API 33+ (band shader); desktop uses displaced UVs, everywhere else there is no band.
- Desktop has no live saturation and no superellipse corners.
- Cross-window blur has no colour filter, and devices or battery saver can refuse it.
