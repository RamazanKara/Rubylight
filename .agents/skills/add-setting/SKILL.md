---
name: add-setting
description: Add, modify, or retire a Rubylight host setting or Nightjar client setting across config, UI schema, locales, docs, and tests.
---

# Add or Change a Setting

Use this skill when adding, changing, renaming, or retiring a setting in Rubylight (host) or Nightjar (Android + desktop).

## Core rules

- **Never read raw `Config` inside `rust/host/stream/src/stream/**`.** Always read effective stream settings through `rust/host/stream/src/stream/config.rs` so override precedence (`app + device > app > device > host > preset`) applies (`tools/arch_check.py` enforces `raw-config-read`).
- **Never drop a key silently.** Rename or retire keys in the `[retired]` table of `rust/core/config/settings.toml` (host; generates `RENAMED_KEYS` / `REMOVED_KEYS`), or in `[[retired]]` / `[[renamed]]` of `client/protocol/policy/settings.toml` plus `migrate` in `client/protocol/desktop-config/src/config/migrate.rs` (desktop; serde aliases for enum values) or `client/app/src/main/java/com/limelight/preferences/PreferenceMigrations.java` and `SettingsKeysTest` (Android).
- **Never hand-edit generated files** (`*_gen.rs`, `*.gen.ts`, `SettingsCatalogGen.java`, generated doc blocks): edit the `settings.toml` catalog and regenerate.
- **Copy style:** State what the user gets, what it costs (delay, quality, network, load, or what breaks), and when to change it ("Leave it on unless ..."). Put break-streaming warnings first. Never label shipping features "Experimental".
- **Fewer settings on Nightjar:** Prefer a safe measured default over a new switch; ship Android and desktop parity in one change.

## Host setting checklist

1. **Scaffold the entry** (optional): prints a `[[setting]]` entry, order-list checklist, i18n spec and note; `--group` is required; `--settings-30` (implies `--extension`), `--override-allowed`, `--restart` and `--extension` set the matching fields, `--vendor amd|nvidia` picks the encoder console group:
   ```powershell
   python tools/scaffold.py setting <key> --type bool --default true --group "..." --label "..." --description "..." --dry-run
   ```
2. **Catalog:** Add the `[[setting]]` entry (key, part, type, default, UI text, control) in `rust/core/config/settings.toml`. For a no-JS console row add its `console` field and insert the key in `[console]` `global_order` next to the other keys of its console group (groups are contiguous; app/device rows: `[[console.app]]` / `[[console.client]]`).
   - Docs table: add the key to the `keys` of the `[[docs.sections]]` entry matching its category; these lists alone drive the All settings tables in `docs/configuration.md`.
   - Counts: raise the hard-coded counts in `rust/core/config/src/config/tests/catalog_parity.rs` and `parts.length` in `rust/web/SettingsSchema.test.mjs` (a 3.0 setting: also `pending = true` and `pending.length`, or `LIVE`/`live` once it ships, and `PER_DEVICE` when overridable).
   - 3.0 extension: `settings_30` fields and `[rust]` `settings_30_order` (generates `SETTINGS_30`; update the count test in `rust/core/config/src/config/settings30.rs`).
   - Per-app/device override: `override_allowed` and `[rust]` `override_order` (read by `override_allowed` in `rust/core/config/src/config/overrides.rs`; resolution lives in `rust/core/session/src/device_overrides.rs`).
   - Read only at startup: `restart` and `[rust]` `restart_order`, plus the hand-written `RESTART_KEYS` in `rust/host/web/src/web/config.rs`.
   - Tiers: the `[web]` lists (everyday settings need none and may not depend on diagnostic or Games-page ones; `folded_groups` generates `FOLDED_GROUPS`).
3. **Accessor & validation:** Use the entry's `rust_accessor` / `validate` fields; hand-write only what they cannot express, on `Config` in `rust/core/config/src/config.rs` (parsers in `rust/core/config/src/config/values.rs`). Add tests in `rust/core/config/src/config/tests.rs` or `rust/core/config/src/config/tests/`.
4. **Regenerate:** `python tools/generated_docs.py` writes the Rust catalogs, the modules in `rust/web/src/lib/schema/`, the console rows rendered by `rust/host/web/src/console/settings.rs` and the All settings tables in `docs/configuration.md`.
5. **Host locales:** Add translations across all 22 `rust/assets/locale/*.json` files via:
   ```powershell
   python tools/i18n.py apply <spec.json> --dry-run
   python tools/i18n.py apply <spec.json>
   ```
6. **Docs & release note:** Update the hand-written topic section in `docs/configuration.md` (search/slice — never read whole) and add a user-facing bullet fragment in `rust/release/notes.d/<topic>.md`.

## Nightjar setting checklist (Desktop + Android)

1. **Shared policy:** Put any decision shared by both apps in `client/protocol/policy/src/`.
2. **Catalog:** Add `[[desktop_setting]]` and `[[android_setting]]` entries (linked by `android_key` / `desktop_key`) in `client/protocol/policy/settings.toml`.
3. **Desktop:** Add the field to `Settings` in `client/protocol/desktop-config/src/config.rs` (enums in `client/protocol/desktop-config/src/config/enums.rs`; default and override list come from the catalog), a row under `client/protocol/desktop-ui/src/ui/settings/` with its key in `SECTION_KEYS` in `client/protocol/desktop-ui/src/ui/settings.rs`, tests in `client/protocol/desktop-ui/src/ui/settings_tests.rs` or `client/protocol/desktop-ui/src/ui/settings_tests/`, and all 14 tables in `client/protocol/desktop-config/src/strings_70/*.rs` (the frozen `strings/*.rs` never grow) via `tools/i18n.py apply`.
4. **Android:** Add the preference in `client/app/src/main/res/xml/preferences.xml` (main rows also in `client/app/src/main/res/values/settings_tiers.xml`), reading in `client/app/src/main/java/com/limelight/preferences/PreferenceReader.java`, and strings across `client/app/src/main/res/values*/` via `tools/i18n.py apply` (cross-fills shared Desktop<->Android strings automatically).
5. **Regenerate:** `python client/scripts/generate-settings.py` writes `client/protocol/policy/src/settings_catalog_gen.rs`, `client/protocol/desktop-config/src/config/defaults_gen.rs`, `client/app/src/main/java/com/limelight/preferences/SettingsCatalogGen.java` and `client/docs/settings.md`. Parity tests: `client/protocol/policy/tests/settings_catalog.rs`, `client/protocol/desktop-config/tests/settings_catalog_parity.rs`, `client/scripts/test_generate_settings.py`.

## Verification

Always set `$env:TEMP="D:\t"; $env:TMP="D:\t"` and run Cargo with `tools/dev.py cargo` and wrap Web builds with `tools/build_lock.py`:

```powershell
python tools/dev.py cargo -- test -p butterpollo-config --locked
python tools/build_lock.py --lane web -- pwsh -c "cd rust/web; node --test SettingsSchema.test.mjs SettingsHome.test.mjs SettingsTiers.test.mjs"
python tools/dev.py cargo -- test --manifest-path client/protocol/Cargo.toml -p nightjar-policy --test settings_catalog --locked
python tools/dev.py cargo -- test --manifest-path client/protocol/Cargo.toml -p nightjar-desktop-config --test settings_catalog_parity --locked
python tools/dev.py cargo -- test --manifest-path client/protocol/Cargo.toml -p nightjar-desktop-ui settings --locked
python client/scripts/test_generate_settings.py
python tools/generated_docs.py --check
python tools/i18n.py check
python tools/arch_check.py
python tools/code_map.py check
tools\dev.ps1 check
```
