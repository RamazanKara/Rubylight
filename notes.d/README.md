# Release-note fragments (Nightjar)

Write `client/notes.d/<topic>.md`, one file per topic, instead of editing the release text or the fastlane changelogs, so parallel branches never conflict on them. A fragment has an English and a German section, each a list of short, user-facing bullets (no Markdown beyond the bullets, links or `**bold**`; the store changelogs get plain text):

```markdown
## en
- The stream menu opens with a long press on the screen.

## de
- Das Stream-Menü öffnet sich mit langem Drücken auf den Bildschirm.
```

At release time, after bumping `versionName` and `versionCode` in `client/app/build.gradle`:

```powershell
python tools\nightjar_notes.py --out <scratch>\whats-new.md
```

writes the "What's new" draft for `gh release edit --notes-file` (add the "Tested." paragraph) and `client/fastlane/metadata/android/{en-US,de}/changelogs/<versionCode>.txt`, then deletes the fragments. It fails, changing nothing, when a fragment lacks a language, a changelog would exceed Google Play's 500 characters, or the changelog for that versionCode already exists (`--force` replaces it). `--keep` keeps the fragments. This README stays.
