# Translations

[Documentation](index.md)

Nightjar ships in 15 languages. English (`values`) is the source; every other language must translate every string before it can ship, because lint treats a missing translation as an error.

## Languages

| Language | Resource folder | Origin | Status |
| --- | --- | --- | --- |
| English | `values` | Source text | Maintained |
| German (Deutsch) | `values-de` | Written for Nightjar | Maintained |
| Chinese, Simplified (简体中文) | `values-zh-rCN` | 23 strings reused unchanged from the Moonlight/Artemis community translation, 56 more adapted from it; the rest machine-assisted | Community review wanted |
| Chinese, Traditional (繁體中文) | `values-zh-rTW` | 61 strings reused unchanged from the Moonlight/Artemis community translation, 18 more adapted from it; the rest machine-assisted | Community review wanted |
| French (Français) | `values-fr` | 51 strings reused unchanged from the Moonlight/Artemis community translation, 28 more adapted from it; the rest machine-assisted | Community review wanted |
| Italian (Italiano) | `values-it` | 40 strings reused unchanged from the Moonlight/Artemis community translation, 39 more adapted from it; the rest machine-assisted | Community review wanted |
| Japanese (日本語) | `values-ja` | 16 strings reused unchanged from the Moonlight/Artemis community translation, 38 more adapted from it; the rest machine-assisted | Community review wanted |
| Korean (한국어) | `values-ko` | 42 strings reused unchanged from the Moonlight/Artemis community translation, 37 more adapted from it; the rest machine-assisted | Community review wanted |
| Polish (Polski) | `values-pl` | 45 strings reused unchanged from the Moonlight/Artemis community translation, 34 more adapted from it; the rest machine-assisted | Community review wanted |
| Portuguese, Brazil (Português (Brasil)) | `values-pt-rBR` | 43 strings reused unchanged from the Moonlight/Artemis community translation, 35 more adapted from it; the rest machine-assisted | Community review wanted |
| Russian (Русский) | `values-ru` | 50 strings reused unchanged from the Moonlight/Artemis community translation, 29 more adapted from it; the rest machine-assisted | Community review wanted |
| Spanish (Español) | `values-es` | 39 strings reused unchanged from the Moonlight/Artemis community translation, 40 more adapted from it; the rest machine-assisted | Community review wanted |
| Turkish (Türkçe) | `values-tr` | 53 strings reused unchanged from the Moonlight/Artemis community translation, 26 more adapted from it; the rest machine-assisted | Community review wanted |
| Ukrainian (Українська) | `values-uk` | 23 strings reused unchanged from the Moonlight/Artemis community translation, 56 more adapted from it; the rest machine-assisted | Community review wanted |
| Vietnamese (Tiếng Việt) | `values-vi` | 61 strings reused unchanged from the Moonlight/Artemis community translation, 18 more adapted from it; the rest machine-assisted | Community review wanted |

Each language has 1,718 strings, counting each plural and string array as one; `python scripts/translation-status.py` prints the current count. "Machine-assisted" means the text was drafted by translation tooling, checked against the rules below and reviewed for terminology, but not yet read by a native speaker. Corrections from native speakers are the most useful contribution for these languages.

The reused sentences come from the community translations of [Moonlight Android](https://github.com/moonlight-stream/moonlight-android) and [Artemis Android](https://github.com/ClassicOldSong/moonlight-android) (both GPL-3.0, like Nightjar). A sentence was reused only where its English source was identical to Nightjar's, with the app name replaced; those projects' translations also set the terminology for each language.

The in-app help (Settings → App → Help, or Help on the home screen) is built from `strings_help.xml`, so it is translated with the rest of the app and works offline in every language. The linked web guides stay in English.

## Help translate

You don't need to build the app to translate it.

- **Fix a string:** find the key in `app/src/main/res/values/` (English) and the same key in `app/src/main/res/values-<language>/`, edit it and open a pull request. Say which screen you looked at if you could.
- **Review a language:** use Nightjar in that language and report wording that is wrong, unnatural or cut off in an issue titled `Translation (<language>): …`, with a screenshot.
- **Add a language:** open an issue first. A new language needs all strings translated (see [Adding a language](#adding-a-language)).
- **Weblate:** a hosted Weblate project is planned; until it exists, use pull requests.

Run `python scripts/translation-status.py` to list missing or stale keys per language.

## Rules

- Keep **Nightjar** and product names (Sunshine, Apollo, Vibepollo, Rubylight, GFE, Moonlight, Tailscale, PyroWave, ES-DE) untranslated.
- Keep keyboard keys (Ctrl, Alt, Shift, Esc) and controller buttons (A/B/X/Y, LB, RB, Start, Guide) as printed on the hardware.
- Use the terms Android uses in your language, and one term per concept throughout.
- Keep every placeholder: `%1$s`, `%2$d`, `%d`, `%1$.1f`, `%%`. Positional placeholders may move; plural items keep the number.
- Escape apostrophes as `\'` or use the typographic ’; escape double quotes as `\"` or use your language's quotation marks. Keep `\n`, `&amp;` and inline tags such as `<b>` intact.
- Give plurals every quantity your language needs (for example `one`, `few`, `many` and `other` in Russian, Ukrainian and Polish; only `other` in Chinese, Japanese, Korean and Vietnamese).

`TranslationResourcesTest` (a JVM unit test run in CI) enforces these checks for every `values-*` folder:

1. **Placeholders** must match the English string. Plural items are compared with the English `other` item. Strings marked `formatted="false"` are skipped.
2. **No bare apostrophes or double quotes**, in English or any translation.
3. **Tight labels** (buttons and menu entries whose layout leaves little room, listed in `TIGHT_LABELS`) may be at most twice as wide as the English label, or 10 columns wider, whichever allows more. Chinese, Japanese and Korean characters count as two columns. Add a key to the list when a new button or menu entry has a fixed width. The preset chips in Settings (`stream_preset_names`) are not checked but ellipsize on narrow screens; keep them to about 20 characters.

`ResourceConfigurationTest` checks that every translated folder is offered in both language pickers and packaged by `localeFilters`.

## Adding a language

1. Create `app/src/main/res/values-<code>/` (for example `values-nl` or `values-pt-rBR`) with every file from `values/` that holds translatable strings.
2. Add the language to `app/src/main/res/xml/locales_config.xml` (BCP 47 code, for example `pt-BR`), to `language_names` (its own name for itself) and `language_values` in `values/arrays.xml`, to `language_names` in every translated `arrays.xml`, and to `localeFilters` in `app/build.gradle`.
3. Run `python scripts/generate-settings.py` and the unit tests.

## Weblate setup (owner step)

The project owner creates the hosted Weblate project (`nightjar`, free libre hosting) and connects it to this repository; no account exists yet. `.weblate` in the repository root points the `wlc` client at it.

Strings are split across several files per language, so use one component per file through the **Component discovery** add-on:

| Setting | Value |
| --- | --- |
| File format | Android String Resource |
| Regular expression to match translation files | `app/src/main/res/values-(?P<language>[a-z]{2}(-r[A-Z]{2})?)/(?P<component>[^/]+)\.xml` |
| Customize the component name | `{{ component }}` |
| Define the monolingual base filename | `app/src/main/res/values/{{ component }}.xml` |
| Define the base file for new translations | `app/src/main/res/values/{{ component }}.xml` |
| Push branch | `weblate` (merge through pull requests so CI runs the checks above) |

Then add the **Squash Git commits** add-on, add `java-printf-format` to the components' translation flags so Weblate checks placeholders, and set the source language to English. Files in `values/` without translatable strings (colors, dimensions, styles, ids) have no translated counterpart and are not discovered.
