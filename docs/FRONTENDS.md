# Launcher integrations

[Documentation](index.md)

Nightjar can stream a game opened from Beacon, ES-DE, Daijisho, Pegasus or another Android launcher. Exported `.art` entries identify the paired PC and app; the launcher opens the entry with Nightjar. No PC-side export script or manually copied game IDs are needed.

## Export and update games

1. Pair the PC in Nightjar and open its library once to cache the current app list.
2. Open **PC settings → Add games to a launcher** and choose your launcher.
3. Pick a ROMs folder. Nightjar creates a `butterpollo` subfolder containing one `.art` entry per visible game. Use that subfolder as the launcher's game folder.
4. For ES-DE, optionally select its configuration folder as well. Other launchers go straight to export.
5. Follow the setup instructions shown when export finishes. **Copy launch command** copies a command for the installed Nightjar package, including root or debug builds.

Nightjar remembers each successful export's folders separately for each PC and launcher, and from then on keeps that export in sync by itself: whenever it sees the PC's games change (fetched while the PC's library is open) or a game is hidden or shown, it writes the export again a few seconds later in the background. Unchanged lists write nothing. The PC's settings show **Kept in sync** with the launchers; the launcher's menu offers **Update games** to export at once, **Pick folder** to change the destination, **Setup instructions** to reopen the guide and command, and **Stop syncing** to forget the destination (the exported files stay). If folder access expires or the SD card is absent, syncing stops with a notice, the PC's settings say so, and the launcher's menu offers the folder picker again.

On Android 11+, select the actual ROMs/ES-DE directories rather than a restricted storage root. The folder provider must permit creating and updating files. Devices without a system folder picker cannot export locally; Android home-screen shortcuts remain available when the home launcher supports them.

## Beacon

1. Export with **Beacon** selected in Nightjar.
2. In Beacon, add a **Custom** platform. Choose **Nightjar** as **Player app** and the exported `butterpollo` subfolder as **ROMs folder**. Include `.art` in the scanned extensions.
3. Under **Advanced**, select **Default** file handling, enable **Use custom launch**, and paste Nightjar's copied command:

```text
am start -n com.butterpollo.client/com.limelight.ShortcutTrampoline -a android.intent.action.VIEW --grant-read-uri-permission -d {file_uri}
```

4. Save and scan the platform. Selecting a game opens Nightjar and starts that game on its paired PC.

Keep Beacon's `{file_uri}` placeholder exactly as written; its underscore differs from Daijisho/Pegasus. The URI and read grant let Nightjar open the entry without broad storage permissions. Use the **Custom** platform and command rather than Beacon's built-in Moonlight player: these are self-contained `.art` entries, not numeric `.moonlight` files or a `Moonlight.uuid` setup. Existing `.moonlight` exports need to be re-exported through Nightjar.

The custom-command placeholders are also documented by [Bannerlator's Beacon integration](https://github.com/moukrea/Bannerlator-AutoSetup/blob/main/marcescence-frontends.md). Launcher field labels may vary with its language/version.

## ES-DE

1. Pair the PC in Nightjar and open its library once to refresh the cached app list.
2. Long-press the PC, choose **PC settings…**, then **Add games to a launcher → ES-DE**.
3. Select the ROMs folder used by ES-DE. The exporter creates a `butterpollo` subfolder containing a game entry for each visible app.
4. Select the ES-DE folder to merge the custom system and copy cached covers. Skip this step when exporting only game files for another frontend.
5. Restart ES-DE and open its **Nightjar** system.

Entries are matched by PC and app identity and updated in place even if a game was renamed; the existing filename and matching cover filename stay stable. Same-name games from different PCs are disambiguated. This behavior also applies to exports for the other launchers.

An unsuccessful new entry or cover write is removed when the document provider permits it, so a retry does not leave a broken duplicate or empty cover. Existing custom covers and files belonging to other PCs are preserved. Every export (by hand or in sync) removes the entries it wrote for this PC's games that were removed or hidden, and the covers Nightjar wrote for them. Entries it writes carry `[nightjar_export] 1`; an unmarked entry counts as the export's only when it is exactly what an earlier export wrote for one game. Entries you wrote (only a PC, a game by name, or edited), other files, other PCs' entries, and covers Nightjar did not write or that changed since (by you or a scraper) are never touched. Covers Nightjar wrote are replaced when the PC's art changes, and covers the library never loaded are fetched from the PC. Exports never empty the folder when no game list is cached. If storage permission or a provider failure prevents cleanup, remove the incomplete file before retrying.

The system folder and emulator name stay `butterpollo` and `BUTTERPOLLO` from the app's earlier name, so existing ROM and media folders keep working; ES-DE shows the system as **Nightjar**. The exporter uses these literal paths and identifiers:

| Relative to selected folder | Contents |
| --- | --- |
| ROMs: `butterpollo/*.art` | Game entries; hidden apps are omitted. |
| ES-DE: `custom_systems/es_systems.xml` | Merged Nightjar system definition; other systems are retained. |
| ES-DE: `custom_systems/es_find_rules.xml` | Android component lookup. |
| ES-DE: `downloaded_media/butterpollo/covers/` | Available cached game covers. |

The generated system uses `%ROMPATH%/butterpollo`, extensions `.art .ART`, the `pc` platform and `windows` theme. Its command is:

```text
%EMULATOR_BUTTERPOLLO% %ACTIVITY_CLEAR_TASK% %ACTIVITY_CLEAR_TOP% %ACTION%=android.intent.action.VIEW %DATA%=%ROMPROVIDER%
```

The Android component is `com.butterpollo.client/com.limelight.ShortcutTrampoline`. The root flavor substitutes `com.butterpollo.client.root` for the package.

## Daijisho

Create a platform for `.art` files and point it at the exported ROMs subfolder. Configure its Android player with the following intent arguments:

```text
-n com.butterpollo.client/com.limelight.ShortcutTrampoline -a android.intent.action.VIEW --grant-read-uri-permission -d {file.uri}
```

The frontend must provide a readable file/content URI and grant read access to it.

## Pegasus

Use the exported entries with an Android launch command:

```text
launch: am start -n com.butterpollo.client/com.limelight.ShortcutTrampoline -a android.intent.action.VIEW --grant-read-uri-permission -d {file.uri}
```

`{file.uri}` is supplied by the frontend, not entered as a literal address.

## Entry format and direct intents

A game entry is plain UTF-8 text. This example uses synthetic identifiers:

```text
# Nightjar game entry
[host_uuid] 11111111-1111-1111-1111-111111111111
[host_name] Gaming PC
[app_uuid] 22222222-2222-2222-2222-222222222222
[app_name] Example game
[app_id] 1234567
[nightjar_export] 1
```

`[nightjar_export] 1` marks an entry Nightjar's export wrote and may update or remove; leave it out of entries you write yourself.

The host UUID is preferred over its name. The app UUID is preferred over a numeric ID, then an app name; name lookup uses the known app list. Entries without app fields open the PC library.

A launcher can also target the same component with these string extras instead of a file:

| Extra | Meaning |
| --- | --- |
| `UUID` | Paired host UUID. |
| `Name` | Host name if UUID is absent. |
| `AppUuid` | Preferred app UUID. |
| `AppId` | Numeric application ID. |
| `AppName` | App name to resolve from the known library. |

The parser is [FrontendEntry.java](../app/src/main/java/com/limelight/utils/FrontendEntry.java), export paths/commands are in [FrontendExporter.java](../app/src/main/java/com/limelight/utils/FrontendExporter.java), and intent handling is in [ShortcutTrampoline.java](../app/src/main/java/com/limelight/ShortcutTrampoline.java) and the [manifest](../app/src/main/AndroidManifest.xml).

## Fix an integration

| Symptom | Cause | Fix |
| --- | --- | --- |
| Game file cannot be opened | Invalid entry or unreadable URI. | Re-export; make sure the launcher grants read access. Keep `%ROMPROVIDER%` in the ES-DE command. |
| PC not found | PC is not saved on this Android installation. | Pair it here and export again. |
| Library opens instead of the game | Entry has no usable app fields. | Open the PC library to refresh it, then re-export. |
| Beacon opens the wrong app or rejects the file | Built-in Moonlight player, wrong placeholder or missing read grant. | Select Nightjar on a Custom platform, scan `.art`, and paste Nightjar's copied command with `{file_uri}` and `--grant-read-uri-permission`. |
| Update games fails after working previously | SD card absent, folder moved or access revoked. | Restore the card or choose **Pick folder** and grant access again. |
| Nightjar system absent in ES-DE | Configuration folder was skipped/wrong or ES-DE has not reloaded it. | Select the correct ES-DE folder, check both custom XML files, then restart ES-DE. |
