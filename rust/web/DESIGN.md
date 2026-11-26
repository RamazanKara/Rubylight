# Rubylight 7.0 design system

One visual language for every host surface: the console, `/play`, `/guest`, the scriptless console, the couch menu, the tray and setup. Two themes, **Obsidian Studio** (dark) and **Alabaster Mineral** (light), share one ruby accent; anything drawn over game art or video uses the **media** palette, which is dark in both. Depth comes from planes, not decoration: the page (`--bg`), panels on it (`--panel`), raised layers above (`--panel-raised`) and wells below (`--sunken`, `--field`). A dark plane carries a top-lit edge (`--edge`), a light plane a soft shadow. Status is always a word as well as a colour. Every page builds from these tokens and components; a new raw value or a one-off component needs a reason in its commit.

- Tokens: [src/styles/tokens.css](src/styles/tokens.css). Checked by [Tokens.test.mjs](Tokens.test.mjs).
- Components: [src/components/](src/components/). Checked by [Components.test.mjs](Components.test.mjs) and, with Playwright, [Design.browser.cjs](Design.browser.cjs).
- Gallery: `npm run dev`, then open `/design.html` (dev only, not in the build).
- Screenshots: `PLAYWRIGHT_MODULE=<playwright> CHROMIUM_EXECUTABLE=<chrome.exe> node scripts/screenshots.cjs <dir> <label>` writes each page in light and dark, desktop and phone, against fixture data. It never contacts a host. `SHOT_THEMES=dark` and `SHOT_FORMS=360,768,1440` pick themes and widths (a width under 900 shoots the full page), `SHOT_PAGES=/,/devices` picks pages (`/design.html` is the gallery), `SHOT_FULL=1` shoots full desktop pages, `SHOT_AUTH=signed-out` or `SHOT_AUTH=setup` shows the sign-in or first-run setup page, `SHOT_CLICK=Select,"Select all"` clicks those buttons before each shot, `SHOT_CONFIG='{"amd_usage":"auto"}'` adds saved settings to the fixture, `SHOT_COOP=missing` (or `failed`, `blocked`, `notice`) sets Nucleus Co-op's folder state, and `SHOT_SESSIONS=live` (or `empty`) puts a running stream on the Sessions page (or clears its history).
- Nightjar has its own design numbers and amber palette: [client/design/tokens.json](../../client/design/tokens.json), generated into Android, Rust and CSS by `client/scripts/gen_design_tokens.py` ([client/design/README.md](../../client/design/README.md)).

## Tokens

`src/styles/tokens.css` is the only file with raw values. [RawCssValues.test.mjs](RawCssValues.test.mjs) reads every stylesheet, `<style>` block and inline style and fails on a raw colour, shadow, radius, layer or duration anywhere else. The host's other surfaces mirror the palette by value: `rust/host/web/src/console/style.css`, the couch menu's theme, the tray's `status.rs` and setup's `ui_theme.rs`, each with a test against this file.

| Group | Tokens | Rule |
|---|---|---|
| Type | `--text-xs` 13, `--text-sm` 14, `--text` 16, `--text-md` 18, `--text-lg` 22, `--text-xl` 28, `--text-2xl` 40, `--text-hero` 48 | Body is 16. Labels and buttons 14, hints 13. One h1 per page at 2xl. Hero is for display figures and the pairing PIN only. |
| Leading | `--leading-tight` 1.15, `--leading-snug` 1.4, `--leading` 1.5 | Headings tight, controls snug, prose normal. |
| Weight | `--weight-regular` 400, `--weight-medium` 560, `--weight-semibold` 600, `--weight-bold` 680 | Medium for labels and buttons, semibold for emphasis and badges, bold for headings. |
| Tracking | `--tracking-display` -0.025em, `--tracking-tight` -0.01em, `--tracking-label` 0.08em | Page and stage titles, section titles, uppercase eyebrow labels (`.eyebrow`). |
| Fonts | `--font`, `--font-display`, `--mono` | Segoe UI Variable; Cascadia Mono with tabular figures for addresses, paths, keys and numbers (`.num`). |
| Space | `--space-1`..`--space-8`: 4, 8, 12, 16, 24, 32, 48, 64 | Gaps inside a control 8, between fields 16-24, between panels 24, page margins 32-48. |
| Shape | `--radius-xs` 4, `--radius-sm` 8, `--radius` 12, `--radius-lg` 16, `--radius-xl` 24, `--radius-pill` | Key caps 4, chips 8, controls and notices 12, panels, cards and dialogs 16, stages 24, badges and switches pill. |
| Hit target | `--control-height` 44 | Every clickable thing is at least 44 x 44. |
| Shell | `--nav-width` 264, `--bar-height` 64 | The rail and the command bar above the page. |
| Depth | `--edge`, `--shadow`, `--shadow-raised`, `--shadow-pop`, `--shadow-accent` | A plane is `box-shadow: var(--edge), var(--shadow)`: the edge shows in dark, the shadow in light. Raised for hover and menus, pop for dialogs and toasts, accent under a hovered primary action. |
| Rings and marks | `--ring-accent`, `--ring-focus`, `--ring-hollow`, `--ring-media`, `--mark-accent`, `--mark-danger`, `--mark-tab`, `--rule-above`, `--rule-grid` | Fixed geometry in token colours, so a component never writes a shadow of its own. |
| Layers | `--z-below`, `--z-base`, `--z-raised`, `--z-bar`, `--z-sticky`, `--z-dock`, `--z-overlay`, `--z-toast`, `--z-skip` | Ascending in that order. |
| Focus | `--focus`, `--focus-width` 2, `--focus-offset` 2 | `:focus-visible` everywhere, never removed without a replacement. Over media use `--media-focus`. |
| Motion | `--duration-fast` 120 ms, `--duration-base` 200 ms, `--duration-slow` 320 ms; `--ease-standard`, `--ease-enter`, `--ease-exit`, `--ease-spring`; `--pulse-duration`, `--spin-duration`, `--dwell-duration` | Hover and toggles fast, reveals and dialogs base, page changes slow; spring only for switch knobs and focus frames. All durations are 0 under `prefers-reduced-motion`, and pulses and spinners stop. Animate opacity and transform only. |

### Colour

| Role | Tokens | Use |
|---|---|---|
| Surfaces | `--bg`, `--panel`, `--panel-raised`, `--sunken`, `--field` | Page, panels and cards, dialogs and menus, wells and hovers, text fields (paper in light, a recessed well in dark). |
| Text | `--ink`, `--ink-2`, `--muted` | Primary, secondary, hints and labels. |
| Lines | `--line`, `--line-strong` | Dividers and panel borders, control borders. |
| Accent | `--accent`, `--accent-strong`, `--accent-ink`, `--accent-soft`, `--accent-line`, `--accent-text` | Ruby: one primary action per view, selection, the on switch, the current page in the rail. Text in the accent colour uses `--accent-text`. |
| Status | `--ok`, `--warn`, `--danger`, each with `-soft` and `-line`; `--live` | Text and icons in the status colour on its soft tint with its line as the border, or on a surface. |
| Washes | `--wash`, `--wash-accent` | A panel that leads a page: a quiet vertical wash, or a ruby one from its top corner. |
| Media | `--media-bg`, `--media-panel`, `--media-panel-raised`, `--media-ink`, `--media-ink-2`, `--media-line`, `--media-idle`, `--media-ok`, `--media-warn`, `--media-danger`, `--media-accent` (+ `-strong`, `-ink`, `-text`), `--media-focus`, `--media-viz-1`..`-4`, `--media-scrim` (+ `-soft`, `-strong`), `--media-wash`, `--media-wash-side`, `--media-glow`, `--media-edge`, `--media-shadow`, `--media-black` | The live stage, game headers, `/play` and `/guest`: text and controls over game art and video; dark in both themes. The four `viz` colours are the pipeline stages and telemetry traces, in order: capture, encode, network, client. |
| Brand | `--brand`, `--brand-ink` | The app icon's fixed ruby and the mark on it (`BrandMark`), the same in both themes. Actions use the accent, not these. |
| Fixed | `--qr-paper`, `--qr-ink`, `--mask-solid` | A QR code is black on white in both themes; the opaque stop of a fade mask. |
| Other | `--skeleton`, `--scrim` | Loading placeholders, dialog backdrops. |

Every text colour reaches 4.5:1 on every surface it is drawn on, in both themes and on the media planes; focus rings, control outlines and the `viz` colours reach 3:1. `Tokens.test.mjs` lists the pairs: add a pair there when you draw text on a new background. The `system` dark block must equal the `dark` block (the test checks it).

## Shell

The console is a rail and a page column. The rail (`--nav-width`) holds the brand, the everyday pages, Advanced (folded) and, at its foot, this PC's card (name, address with a copy button, version) and Sign out; the current page is a tinted row with a ruby marker at its leading edge. Above the page sits the command bar (`--bar-height`): Search (opens the quick switcher, Ctrl+K), the live badge (`components/shell/LiveBadge`: the running game, or "Live" while a device streams; it links to Home) and the theme switch (`components/shell/ThemeSwitch`). Below 860 px the rail becomes a sticky top bar with Search and a menu; the theme switch moves into the menu and a beating dot beside Home stands in for the live badge. The bar scrolls with the page, so a page's own sticky parts keep their offsets.

## Components

| Component | Use it for | Notes |
|---|---|---|
| `Button` | Actions. `primary` (one per view), `secondary`, `ghost`, `danger`; `busy`; `href` makes a link. | Label says the outcome ("Unpair", not "OK"). |
| `Panel` | A titled page section; `flush` for tables and lists, `collapsible` for advanced. | |
| `Card` | One item in a collection (device, app, check); `href` makes the whole card one link, `selected`, `media`, `footer`. | No other controls inside a linked card. |
| `Field` + `Input` / `Select` | A labelled control with hint or error. Give the control and the Field the same `id`; pass `invalid` and `describedby`. | Select is native for keyboard and touch. |
| `Toggle` | A labelled on/off setting (`role="switch"`). `settings/Switch` is the bare variant for setting rows. | |
| `StatusPill` | The state of one thing: idle, ready, streaming, busy, warning, error, offline. | Always shows a word; streaming and busy pulse. The tint carries its status line as the border. |
| `Badge` | Tags and counts that are not states ("HEVC", "Applies after a restart", "New"). | |
| `Banner` | A state or problem on a page or section, with its fix as `actions`. `live` announces it (`alert` for danger). | Toasts (`notify`) are for passing confirmations only. |
| `Dialog` | Confirmation and short focused tasks. Native modal: focus trapped, Escape closes, focus returns. `dismissible={false}` while saving. | For yes/no confirmations call `confirm()` from `lib/feedback`. |
| `Tabs` | Switching views inside one page. One tab stop; arrows, Home, End. | Links between pages use a nav with `aria-current`, like Settings categories. |
| `Table` | Data with columns. Scrolls in a named, focusable region on narrow screens; `numeric` cells align right. | Children are `<thead>` and `<tbody>`. |
| `EmptyState` | Nothing here yet: an icon, a title, one sentence and the next step. | |
| `PageHeader` | The page's h1, a one-line subtitle and up to two `actions`. | Actions sit on the title's line, centred on it; on a phone they go under the subtitle. |
| `BrandMark` | The app icon's mark (white R on `--brand`), the same as the tray, setup and favicon. | Decorative; the name sits beside it. |
| `AuthLayout` | Pages before the console: sign in, setup, cannot connect. One card under the mark. | |
| `Skeleton` | Content loading for more than a moment: text lines, a block or a circle. One `label` per region. | `LoadState` stays for load failures with Retry. |
| `Icon` | The console's 24-pixel stroke icons. | |
| `CopyButton` | Copies a value such as an address or a path and confirms it with a toast. `label` is read out and shown as the tooltip. | |
| `PathPicker` | Browses the host's folders for a program, a file or a folder and fills the text field it wraps. | Escape closes it, like a menu. |

## Rules

- States: every view that loads has a skeleton or loading line, an error with Retry (`LoadState` or a danger `Banner`) and an empty state with the next step.
- Plain language: say what happens and what to do ("No audio output. Turn on a speaker..."), not internal names. Config keys only behind "Show config keys".
- Keyboard: everything reachable with Tab, operable with Enter or Space, visible focus. Composite widgets use `rovingIndex` from `lib/design/keys.ts`.
- Labels: every control has a visible label or an `aria-label`; icons alone get `label`.
- Layout: the page column is at most 1500 px; prose and forms keep to 70 ch; two-column forms collapse below 760 px; no horizontal page scroll at 360 px. Use logical properties (`padding-inline`, `margin-inline-start`, `inset-inline-end`, `text-align: start`).
- Styling: tokens only (`RawCssValues.test.mjs`). Component styles stay scoped; shared classes live in `base.css` (`.num`, `.muted`, `.eyebrow`) and `forms.css` (`.input`, `.select`, `.table`, `.facts`, `.notice`, `.stack`, `.row`).
- Media: nothing but the media palette over game art and video, and no `backdrop-filter` over a playing video (`/play`, `/guest`): a blur there costs frames on the device that decodes.
- Notices: `.notice` is a paragraph, so links and code flow in the sentence. Built from parts (an icon or a `<span>` of text, then a button or link), it lines them up in a row.
- Disclosures: every `<summary>` gets the shared chevron after its label from `base.css` (turns when open, hover colour, focus ring). Don't restyle the marker per page; a summary with its own `<svg>` icon gets no chevron.
- Claims and labels follow the product rules: no "Experimental" anywhere in the UI, never hardware generations in hints.
