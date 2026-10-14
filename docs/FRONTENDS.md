# Launcher integrations

[Documentation](index.md)

Nightjar can stream a game opened from ES-DE, Daijisho, Pegasus or another Android launcher. Exported `.art` entries identify the paired PC and app; the launcher opens the entry with Nightjar.

## ES-DE

1. Pair the PC in Nightjar and open its library once to refresh the cached app list.
2. Long-press the PC, choose **PC settings…**, then **Add games to ES-DE**.
3. Select the ROMs folder used by ES-DE. The exporter creates a `butterpollo` subfolder containing a game entry for each visible app.
4. Select the ES-DE folder to merge the custom system and copy cached covers. Skip this step when exporting only game files for another frontend.
5. Restart ES-DE and open its **Nightjar** system.

On Android 11+, select the actual ROMs/ES-DE directories rather than a restricted storage root. Repeat export after changing the host library. Entries are matched by PC and app identity and updated in place even if a game was renamed; the existing filename and matching cover filename stay stable. Same-name games from different PCs are disambiguated.

An unsuccessful new entry or cover write is removed when the document provider permits it, so a retry does not leave a broken duplicate or empty cover. Existing custom covers and files belonging to other PCs are preserved. Export does not prune entries for games you removed or hid after an earlier export; remove those entries manually in your frontend's ROMs folder. If storage permission or a provider failure prevents cleanup, remove the incomplete file before retrying.

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
-n com.butterpollo.client/com.limelight.ShortcutTrampoline -a android.intent.action.VIEW -d {file.uri}
```

The frontend must provide a readable file/content URI and grant read access to it.

## Pegasus

Use the exported entries with an Android launch command:

```text
launch: am start -n com.butterpollo.client/com.limelight.ShortcutTrampoline -a android.intent.action.VIEW -d {file.uri}
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
```

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
| Nightjar system absent in ES-DE | Configuration folder was skipped/wrong or ES-DE has not reloaded it. | Select the correct ES-DE folder, check both custom XML files, then restart ES-DE. |
