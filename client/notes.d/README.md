# Release-note fragments (Nightjar)

Write `client/notes.d/<topic>.md`, one file per topic, instead of editing the release text or the fastlane changelogs, so parallel branches never conflict on them. A fragment has an English section, a list of short, user-facing bullets (no Markdown beyond the bullets, links or `**bold**`; the store changelog gets plain text). Release notes are English only; the app's own translations stay.

```markdown
## en
- The stream menu opens with a long press on the screen.
```

At release time, after bumping `versionName` and `versionCode` in `client/app/build.gradle`:

```powershell
python tools\nightjar_notes.py --out <scratch>\whats-new.md
```

writes the "What's new" draft for `gh release edit --notes-file` (add the "Tested." paragraph) and `client/fastlane/metadata/android/en-US/changelogs/<versionCode>.txt`, then deletes the fragments. A `## de` section left in an older fragment is ignored. It fails, changing nothing, when a fragment has no English bullets, a changelog would exceed Google Play's 500 characters, or the changelog for that versionCode already exists (`--force` replaces it). `--keep` keeps the fragments. When the bullets are too long together for the store, `--changelog-en <file>` takes a hand-written store changelog instead (checked against the same limit). This README stays.
