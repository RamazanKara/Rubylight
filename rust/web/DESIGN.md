# Rubylight console design system

One visual language for the console, the tray, setup and Nightjar: quiet surfaces, one ruby accent, status in words as well as colour. Every 3.0 page builds from these tokens and components; a new raw value or a one-off component needs a reason in its commit.

- Tokens: [src/styles/tokens.css](src/styles/tokens.css). Checked by [Tokens.test.mjs](Tokens.test.mjs).
- Components: [src/components/](src/components/). Checked by [Components.test.mjs](Components.test.mjs) and, with Playwright, [Design.browser.cjs](Design.browser.cjs).
- Gallery: `npm run dev`, then open `/design.html` (dev only, not in the build).
- Screenshots: `PLAYWRIGHT_MODULE=<playwright> CHROMIUM_EXECUTABLE=<chrome.exe> node scripts/screenshots.cjs <dir> <label>` writes each page in light and dark, desktop and phone, against fixture data. It never contacts a host. `SHOT_PAGES=/,/devices` picks pages (`/design.html` is the gallery), `SHOT_FULL=1` shoots full desktop pages, and `SHOT_AUTH=signed-out` or `SHOT_AUTH=setup` shows the sign-in or first-run setup page.
- Nightjar mapping: `C:\src\butterpollo-release\unified\design-tokens-nightjar.md` (moves to `client/` with the import).

## Tokens

| Group | Tokens | Rule |
|---|---|---|
| Type | `--text-xs` 13, `--text-sm` 14, `--text` 16, `--text-md` 18, `--text-lg` 22, `--text-xl` 28, `--text-2xl` 40 | Body is 16. Labels and buttons 14, hints 13. One h1 per page at 2xl. |
| Leading | `--leading-tight` 1.2, `--leading-snug` 1.4, `--leading` 1.5 | Headings tight, controls snug, prose normal. |
| Weight | `--weight-regular` 400, `--weight-medium` 560, `--weight-semibold` 600, `--weight-bold` 650 | Medium for labels and buttons, semibold for emphasis and badges, bold for headings. |
| Fonts | `--font`, `--font-display`, `--mono` | Segoe UI Variable; Cascadia Mono for addresses, paths, keys and numbers (`.num`). |
| Space | `--space-1`..`--space-8`: 4, 8, 12, 16, 24, 32, 48, 64 | Gaps inside a control 8, between fields 16-24, between panels 24, page margins 32-48. |
| Shape | `--radius-sm` 6, `--radius` 10, `--radius-lg` 12, `--radius-pill` | Controls 10, panels, cards and dialogs 12, badges and switches pill. |
| Hit target | `--control-height` 44 | Every clickable thing is at least 44 x 44. |
| Elevation | `--shadow`, `--shadow-raised`, `--shadow-pop` | Resting cards, hovered cards and menus, dialogs. Dark mode relies on borders, not shadows. |
| Layers | `--z-sticky`, `--z-overlay`, `--z-toast` | |
| Focus | `--focus`, `--focus-width` 2, `--focus-offset` 2 | `:focus-visible` everywhere, never removed without a replacement. |
| Motion | `--duration-fast` 120 ms, `--duration-base` 200 ms, `--duration-slow` 320 ms; `--ease-standard`, `--ease-enter`, `--ease-exit`; `--pulse-duration` | Hover and toggles fast, reveals and dialogs base, page changes slow. All durations are 0 under `prefers-reduced-motion`, and pulses and spinners stop. Animate opacity and transform only. |

### Colour

| Role | Tokens | Use |
|---|---|---|
| Surfaces | `--bg`, `--panel`, `--panel-raised`, `--sunken` | Page, panels and cards, dialogs, wells and hovers. |
| Text | `--ink`, `--ink-2`, `--muted` | Primary, secondary, hints and labels. |
| Lines | `--line`, `--line-strong` | Dividers, control borders. |
| Accent | `--accent`, `--accent-strong`, `--accent-ink`, `--accent-soft`, `--accent-text` | Ruby: one primary action per view, selection, the on switch. Text in the accent colour uses `--accent-text`. |
| Status | `--ok`, `--warn`, `--danger` (+ `-soft`), `--live` | Text and icons in the status colour on its soft tint or a surface. |
| Media | `--media-bg`, `--media-ink`, `--media-ink-2`, `--media-line`, `--media-idle`, `--media-ok`, `--media-warn` | Text over game art and video; dark in both themes. |
| Other | `--skeleton`, `--scrim` | Loading placeholders, dialog backdrops. |

Every text colour reaches 4.5:1 on every surface it is drawn on, in both themes; focus rings and control outlines reach 3:1. `Tokens.test.mjs` lists the pairs: add a pair there when you draw text on a new background. The `system` dark block must equal the `dark` block (the test checks it).

## Components

| Component | Use it for | Notes |
|---|---|---|
| `Button` | Actions. `primary` (one per view), `secondary`, `ghost`, `danger`; `busy`; `href` makes a link. | Label says the outcome ("Unpair", not "OK"). |
| `Panel` | A titled page section; `flush` for tables and lists, `collapsible` for advanced. | |
| `Card` | One item in a collection (device, app, check); `href` makes the whole card one link, `selected`, `media`, `footer`. | No other controls inside a linked card. |
| `Field` + `Input` / `Select` | A labelled control with hint or error. Give the control and the Field the same `id`; pass `invalid` and `describedby`. | Select is native for keyboard and touch. |
| `Toggle` | A labelled on/off setting (`role="switch"`). `settings/Switch` is the bare variant for setting rows. | |
| `StatusPill` | The state of one thing: idle, ready, streaming, busy, warning, error, offline. | Always shows a word; streaming and busy pulse. |
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

## Rules

- States: every view that loads has a skeleton or loading line, an error with Retry (`LoadState` or a danger `Banner`) and an empty state with the next step.
- Plain language: say what happens and what to do ("No audio output. Turn on a speaker..."), not internal names. Config keys only behind "Show config keys".
- Keyboard: everything reachable with Tab, operable with Enter or Space, visible focus. Composite widgets use `rovingIndex` from `lib/design/keys.ts`.
- Labels: every control has a visible label or an `aria-label`; icons alone get `label`.
- Layout: content max 1100-1200 px; two-column forms collapse below 760 px; no horizontal page scroll at 390 px.
- Styling: tokens only. Component styles stay scoped; shared classes live in `forms.css` (`.input`, `.select`, `.table`, `.facts`, `.notice`, `.stack`, `.row`).
- Notices: `.notice` is a paragraph, so links and code flow in the sentence. Built from parts (an icon or a `<span>` of text, then a button or link), it lines them up in a row.
- Disclosures: every `<summary>` gets the shared chevron after its label from `base.css` (turns when open, hover colour, focus ring). Don't restyle the marker per page; a summary with its own `<svg>` icon gets no chevron.
- Claims and labels follow the product rules: no "Experimental" anywhere in the UI, never hardware generations in hints.
