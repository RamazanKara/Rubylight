# Native API v1

[Documentation](README.md) · [HTTP API](api.md) · [Control messages](control-messages.md) · [Hardware metrics](hardware-metrics.md)

The native API lets a paired client go beyond the Moonlight protocol: read the host's capabilities, game library and session state, run session actions, and, with the device's host control permission, read and change Radeon settings. Further features (wake targets, audio processing, files, the image clipboard, shared folders and USB forwarding) are listed in the capabilities response when the host serves them. Version 1 shipped in Rubylight 3.0.0, and Nightjar is the reference client. This page is the contract between the host and Nightjar's `nightjar-core`; [HTTP API](api.md#native-api-v1-paired-clients) has a short summary. Fake-backend host tests and the compatibility matrix in `rust/host/src/nvhttp/native_tests/compat.rs` cover the contract, and the [hardware matrix](hardware-matrix.md) records the Radeon round trip and crash restore on real hardware.

## Ground rules

- **Transport and authentication are reused.** Every endpoint is served by the streaming HTTPS service (`HttpsPort` from `/serverinfo`, normally 47984), next to `/api/hardware-metrics` in `rust/host/src/nvhttp.rs`. The client presents the certificate it paired with, and the host looks it up with `authenticated()` (`client_by_certificate`, enabled and `perm != 0`). Plain HTTP, the admin console port, Basic auth, cookies and API tokens do not apply.
- **Moonlight is unaffected.** `PROTOCOL_VERSION`, `/applist`, `/launch`, `/resume`, `/cancel` and `/appasset` stay as they are. The only change stock clients can see is one more `/serverinfo` field, which they ignore, as they already ignore the `RustHost*` fields.
- **Nothing new on the stream path.** Play and Resume still go through `/launch?appuuid=` and `/resume` with the usual RTSP setup. This API reads state and runs actions. It never carries video, input or per-packet telemetry.
- All responses are `application/json; charset=utf-8` with `Cache-Control: no-store`. POST bodies are at most 16 KiB of JSON, except a file upload (see Files). The host parses the raw body and accepts `application/json`, `text/plain` (nightjar-core's transport sends it for POST) or no content type. An empty body counts as `{}`.

## Discovery and capabilities

`/serverinfo` gets one field:

```xml
<NativeApiVersion>1</NativeApiVersion>
```

| Value | Meaning for the client |
|---|---|
| missing | The host has no native API (Sunshine, Apollo, Vibepollo, Rubylight before 3.0). Show the classic app grid. |
| `0` | The host has it, but not for this request: plain HTTP or an unpaired certificate. Pair first. |
| `1` (or higher) | The client may call `/api/native/v1/capabilities`. A higher number means the host also serves newer versions. v1 stays served (see Versioning). |

This follows `HardwareMetricsVersion` and `NetworkPathVersion`: nightjar-core's `Evidence` stores `-1` for a missing field, and `Feature::NativeApi` comes out Supported, needs `Pair`, or needs `UpdateHost` when the host is Rubylight but the field is missing. A 404 from any `/api/native/v1/` path also means "no native API". The client never decides by version string.

```http
GET /api/native/v1/capabilities
```

```json
{
  "api_version": 1,
  "host": { "name": "GAMING-PC", "version": "3.0.0", "unique_id": "5E0F4B1C-..." },
  "features": ["library", "session", "session.close_game", "session.launcher", "radeon.read", "radeon.write", "radeon.profile"],
  "permissions": { "list": true, "view": true, "launch": true, "host_control": false }
}
```

- `features` says what the host implements and has turned on, the same for every client. `radeon.read`, `radeon.write` and `radeon.profile` appear only where the host started its Radeon worker; `files.upload` and `files.download` only while the host's File transfer setting is on. Clients ignore feature strings they don't know.
- `permissions` belongs to the calling device. `file_upload` and `file_download` (bits `1 << 18` and `1 << 19`) are missing from hosts before file transfer and decode as `false`. `coop_join` is whether the device may join co-op games (Launch apps and its **Can join co-op games** setting, which follows Launch apps until set), never a permission bit; hosts before 3.0 leave it out, which decodes as `false`. With a feature listed but its permission off, the client shows the control disabled, with the sentence from the matching error (for example "Turn on Host control for this device in the Rubylight console").

## Endpoints

All paths start with `/api/native/v1`. "Owner" means the caller owns the current session (see Session).

| Method | Path | Needs | Result |
|---|---|---|---|
| GET | `/serverinfo` (existing) | none | `NativeApiVersion` |
| GET | `/capabilities` | paired | Capabilities |
| GET | `/library` | List apps | Library |
| GET | `/appasset?appid=N&AssetType=2&AssetIdx=0` (existing) | List apps | Cover image |
| GET | `/session` | paired | Session. Details need View or Launch |
| POST | `/session/disconnect` | View or Launch, owner | Session (now `suspended`) |
| POST | `/session/end` | Launch, owner | Session |
| POST | `/session/close-game` | Launch + **host control**, owner, or no session | Session |
| GET | `/launch`, `/resume` (existing) | Launch / View | Play and Resume, unchanged |
| GET | `/radeon` | View or Launch | Radeon |
| POST | `/radeon/settings` | View + **host control**, owner of a `streaming` or `suspended` session | Radeon |
| POST | `/radeon/restore` | View + **host control**, owner | Radeon |
| GET | `/wake` | List, View or Launch | Wake (feature `wake`) |
| GET | `/audio` | View or Launch | Audio (features `audio.night_mode`, `audio.headphone_surround`) |
| POST | `/audio/settings` | View or Launch, owner of a `streaming` or `suspended` session | Audio |
| GET | `/files` | Download files, File transfer on | FileList |
| POST | `/files?name=` | Upload files, File transfer on | 201 Uploaded |
| GET | `/files/content?name=` | Download files, File transfer on | The file's bytes |
| GET | `/folders` | a folder grant | FolderList (feature `folders`) |
| GET | `/folders/{id}/entries?path=` | a grant for `id` | FolderEntries |
| GET | `/folders/{id}/content?path=` | a grant for `id` | The file's bytes |
| PUT | `/folders/{id}/content?path=` | a `write` grant for `id` | 201 Uploaded |
| GET | `/coop` | List apps | Coop: the co-op games and the co-op sessions, with this device's seat, `joinable` and `refusal`; player names only for a device that may join co-op games or holds a seat (feature `coop`; 503 with the reason while Nucleus Co-op cannot run) |
| POST | `/coop/join` `{"session": N}` | Launch; **Can join co-op games** to take another device's free slot | Joined `{"session", "slot", "app_id"}`: the slot is held until this device launches `app_id`. 403 `permission_required`, 409 `no_session` or `coop_refused` |

Permission bits are the existing ones: List `1 << 24`, View `1 << 25`, Launch `1 << 26`, Upload files `1 << 18`, Download files `1 << 19`. Host control is a separate per-device flag (see below).

## Library

```http
GET /api/native/v1/library
```

```json
{
  "revision": "9f2c41d0",
  "games": [
    {
      "install_id": "c6f1a9a4-4b51-5f43-9d0b-7a2b1c2d3e4f",
      "app_id": 1093052211,
      "title": "Dota 2",
      "game_id": "steam:570",
      "store": { "source": "steam", "id": "570", "name": "Steam" },
      "installed": true,
      "running": false,
      "artwork": {
        "cover": "/appasset?appid=1093052211&AssetType=2&AssetIdx=0",
        "version": "a41c09e2"
      }
    },
    {
      "install_id": "8d4b3f55-0c2e-4f8a-9a61-2f1c6f0e9b17",
      "app_id": 881448767,
      "title": "Desktop",
      "game_id": null,
      "store": null,
      "installed": null,
      "running": true,
      "artwork": { "cover": "/appasset?appid=881448767&AssetType=2&AssetIdx=0", "version": "0c1d2e3f" }
    }
  ]
}
```

The entries are the host's configured apps (`h.apps`), in console order. The synthetic Remote session tiles (`remote::identify`) are left out because session actions replace them. There are three ids, each with one job:

| Field | Stable across | Source | Use |
|---|---|---|---|
| `install_id` | renames, reorders, host restarts | the app's `uuid` in `apps.json`. Imports set it to `library::Game::uuid()`: Steam's canonical UUID, otherwise UUIDv5 of `rubylight:library:<source>:<id>` | The client's primary key per host. Launch with `/launch?appuuid=` |
| `game_id` | hosts | `<source>:<store id>` from `launcher-source`/`launcher-id` (or `steam-id`), `null` for manual apps | Merges one title across paired hosts in the client's library, and keys favourites and per-game profiles |
| `app_id` | nothing (it is CRC32 of the name unless pinned) | `App::id()` | Only for `/appasset` and old GameStream calls |

- `store.source` is a string, not a closed enum, so stores added later don't break v1 clients. In 3.0 it is one of `steam`, `epic`, `battlenet`, `ea`, `ubisoft`, `gog`, `microsoft` (`library::Source::key`; `microsoft` covers Microsoft Store and Xbox PC games), and `name` is `Source::name()`.
- `microsoft` (Microsoft Store and Xbox PC games, `rust/windows/src/library/store.rs`): the host lists the packages registered for the signed-in user (`PackageManager`, skipping frameworks, resources and bundles) and takes the ones with a `MicrosoftGame.config` in the package folder or its `Content` folder, which is what a GDK game ships; it also reads `<drive>:\XboxGames\<title>` on fixed drives for a game on disk that no package registers (listed, with the reason, and not launchable). `id` is the config's identity name, the game is started with `shell:AppsFolder\<package family>!<application id>` (the Start menu's way; the application id is the config's `Executable` Id, or the manifest application that runs the same program), and the cover is the config's `Square150x150Logo`. Console-only titles (no executable for PCs) are left out. It starts the package's own entry point and bypasses no licence, sign in or Xbox app check. A Store app that is not a GDK game (a UWP game with no `MicrosoftGame.config`) cannot be told from any other app and is not listed; add it by hand. A failure to list packages shows as a scan error on the source, never as an empty library.
- `installed`: `true` or `false` from the host's last launcher scan (`butterpollo_windows::library::discover`), `null` for manual apps or when no scan has run. The host caches the scan and refreshes it in the background at most every 5 minutes and after console imports. A library request never waits for a scan.
- `running`: the app is the host's current game (`remote_game`), whoever owns it.
- `artwork.cover` is a path on the same HTTPS origin and is fetched with the same certificate. `artwork.version` changes when the image file changes (a hash of its path, size and mtime), so clients can cache covers offline. v1 has covers only.
- `revision` is a hash of the whole response body without `running`. Clients refetch when it changes.

## Session

States: `idle → preparing → launching → streaming → suspended → stopping → idle`.

```http
GET /api/native/v1/session
```

```json
{
  "revision": 42,
  "state": "suspended",
  "owner": "you",
  "app": { "install_id": "c6f1a9a4-...", "app_id": 1093052211, "title": "Dota 2" },
  "game_running": true,
  "since_unix_ms": 1792303200000,
  "suspended_until_unix_ms": 1792310400000,
  "actions": [
    { "id": "resume", "allowed": true },
    { "id": "disconnect", "allowed": false, "reason": "not_streaming" },
    { "id": "end", "allowed": true },
    { "id": "close_game", "allowed": false, "reason": "host_control_required" }
  ]
}
```

| State | Meaning |
|---|---|
| `idle` | No session. `game_running` can still be true after End session |
| `preparing` | Launch accepted: displays and integrations are being prepared (`display_session::prepare_stream`) |
| `launching` | App commands are running, and the stream is not up yet |
| `streaming` | Video is flowing to the owner |
| `suspended` | The owner disconnected. The game and the virtual display stay alive until `suspended_until_unix_ms` (the reconnect grace period) |
| `stopping` | Teardown is in progress. Repeating an action is safe |

`owner` is `none`, `you` or `other_device`. Other devices see `state` and `owner` only: `app` is null and every action is refused with `not_session_owner`, the same scoping `/serverinfo` uses for `currentgame`. Without View or Launch permission, `app` is null as well. The host always names the owner of a session that isn't `idle`: `native::session_view` asserts it in debug builds, since without an owner every caller would read as another device.

The host keeps this state explicitly: `butterpollo_core::session_state::Machine`, fed by `rust/host/src/session_control.rs`. It is a projection of what the host observes plus the transitions the launch and teardown code report, so stock Moonlight flows (`/launch`, `/resume`, `/cancel`, RTSP and ENet disconnects, "Quit app") run unchanged and only record what they do.

| From | To | When |
|---|---|---|
| `idle` | `preparing` | `/launch` or `/resume` of a stream is accepted |
| `preparing` | `launching` | the app's own commands start |
| `launching` | `streaming` | the client connects (RTSP Play) |
| `launching` | `suspended` or `idle` | the launch fails, or nobody joins within 30 s. It returns to what still runs |
| `streaming` | `suspended` | the stream ends (client disconnect, Disconnect action) while the game and its display remain |
| `streaming` | `idle` | the stream ends and no game, or no kept display, remains |
| `suspended`, `streaming` | `preparing` | the owner's `/resume` is accepted (a stream still running is superseded) |
| `stopping` | `preparing` | a `/launch` or `/resume` is accepted during a teardown. It takes over the state, and the teardown ending leaves it alone |
| any but `preparing`, `launching` | `stopping` | End session, Close game, tray Stop, or the grace period running out |
| `launching` | `stopping` | stock `/cancel` while the client has not joined yet (it waits for the launch call to finish first) |
| `stopping` | `idle`, `suspended`, `streaming` | the last teardown ends: what still runs decides |

Teardowns can overlap (End session while the grace period runs out, `/cancel` during either). The state stays `stopping` until the last one has ended. A disconnect or a teardown ending never overwrites a launch in `preparing` or `launching`.

The grace period is the existing disconnect delay (`Config::display_disconnect_delay`): `dd_config_revert_delay` (3 s) when the display layout is exclusive or `dd_config_revert_on_disconnect` is on, otherwise `dd_paused_virtual_display_timeout_secs` (2 h; 0 never expires, and `suspended_until_unix_ms` is then absent). When it runs out, the host ends the session as End session does, and the game keeps running (`idle` with `game_running: true`).

The host writes `session.json` in the profile directory while a session is not `idle` and deletes it when the session ends. A thread of its own writes the file (no `fsync`, retrying a failed write or delete every 2 s), so no launch or request waits for the disk. One found at startup means the host stopped mid-session: it is deleted, logged, and shown as the warning `session_interrupted` for an hour. The host starts `idle`, so recovery is safe to repeat. Display and audio changes of the dead host come back through the existing `display_recovery` journals.

**Actions.** Every action is an empty POST. It is idempotent and returns the Session object after the action has started, often in `stopping`. `disconnect`, `end`, `close_game` and `resume` try the existing `launch_transition` lock and answer `session_busy` when a launch holds it (so a Disconnect cannot remove the fresh launch of the same device), and `end` and `close_game` also when the state is `preparing` or `launching`. They serialize with launches without blocking a request for minutes. `end` and `close_game` wait up to 5 s for the stopped streams, so a handler runs them in `spawn_blocking`.

| Action | Effect | Game | Virtual display | Radeon changes |
|---|---|---|---|---|
| `resume` | The client calls the existing `/resume`, and the host reattaches the suspended session. No native endpoint | kept | reused | kept |
| `disconnect` | Stops the owner's transport now instead of waiting for a timeout. State becomes `suspended` | kept | kept for the grace period | kept |
| `end` | Tears down the owner's stream and the owner's kept display (`request_stop` plus display release). Streams and displays of other devices keep running | **kept running** | released | restored |
| `close_game` | Like `end`, plus ends the game process (`stop_app`, with its undo commands and exit timeout) | ended | released | restored |

When the grace period of a suspended session runs out, the host behaves as for `end`. A disconnect never ends the game. `close_game` also works with no session, when `game_running` is true, for a caller with Launch and host control. That closes a game left behind by End session. Stock Moonlight's `/cancel` is unchanged: it stops every stream and closes the game with Launch permission alone.

Clients poll `/session` about every 2 s while a home or detail screen is visible, and not at all while streaming. The overlay knows the stream is up.

**Launcher prompt (`session.launcher`).** When a launched game does not show up and a Steam, Epic Games, EA app or Ubisoft Connect window is up instead, the owner's Session carries a `launcher` note:

```json
"launcher": { "launcher": "steam", "kind": "update", "window_title": "Updating Dota 2", "since_unix_ms": 1792303212000 }
```

`kind` is `update` (an update, install or download is in the way), `sign_in` (a sign in, a verification code or Steam Guard) or `prompt` (a launcher window in front for more than 20 s with a title that names nothing; a client shows `window_title`). An unknown kind reads as `prompt`. The host reads the visible windows every 2 s while the game has not shown up (`rust/host/src/launcher_watch.rs`, classifier in `rust/core/src/launcher_prompt.rs`). A note needs two looks in a row, goes when two looks in a row find no launcher window, and goes at once when the game's process or window appears. It is a note, not a state: the session state does not change, and the host never clicks or types into a launcher. Only the owner sees it, and an app that starts no program of its own (a desktop tile) or only opens a launcher (Steam Big Picture, a bare `steam.exe`) is not watched, because the launcher is the app. A Playnite-started game counts as seen when Playnite reports it running. A windowed game that the host cannot tell from the launcher can leave a note up until the launcher window closes or the watch's 10-minute limit passes, after which the host stops looking. The fixture is `rust/core/native-api/tests/fixtures/session_launcher.json`.

**Radeon profile (`radeon.profile`).** An app with `radeon-profile` set (see [Radeon settings per game](configuration.md#radeon-settings-per-game)) puts `radeon_profile` in the owner's Session: `profile` (`competitive`, `smoothness`, `quality` or `custom`), `state` (`applying`, `applied` or `failed`), `settings` (the ids written, which go back when the session ends) and, when something was left out or failed, `message`. A failed profile does not stop the launch: the game runs without it, the host log names the setting with `ERROR`, and the session shows `failed` with the reason. The profile goes through the same journaled path as `POST /radeon/settings` and is restored by the same session-end hooks. It needs no `host_control` on the device: the app's config is the consent.

## Radeon state and writes

```http
GET /api/native/v1/radeon
```

```json
{
  "revision": 7,
  "state": "configurable",
  "host_control": true,
  "writable_now": true,
  "gpus": [
    {
      "id": 11520,
      "name": "AMD Radeon RX 7900 XT",
      "driver": "25.9.1",
      "settings": [
        { "id": "anti_lag.enabled", "group": "anti_lag", "kind": "toggle",
          "state": "configurable", "value": true },
        { "id": "chill.max_fps", "group": "chill", "kind": "integer", "unit": "fps",
          "state": "configurable", "value": 144, "range": { "min": 30, "max": 300, "step": 1 },
          "restore": { "original": 120, "written": 144 } },
        { "id": "afmf.enabled", "group": "afmf", "kind": "toggle",
          "state": "needs_external_setup", "value": false,
          "reason": "external_setup", "message": "Turn on AMD Fluid Motion Frames once in AMD Software: Adrenalin Edition." },
        { "id": "rsr.enabled", "group": "rsr", "kind": "toggle", "state": "unsupported" },
        { "id": "image_sharpening.sharpness", "group": "image_sharpening", "kind": "integer", "unit": "%",
          "state": "temporarily_unavailable", "reason": "driver_busy", "message": "The driver is busy. Try again in a moment." }
      ]
    }
  ]
}
```

**The five states.** A client draws exactly what the state says and never shows a toggle that does nothing:

| `state` | `value` | Client shows |
|---|---|---|
| `configurable` | present | An editable control (if `writable_now`) |
| `read_only` | present | The value, not editable. The host's driver probe found it readable but not writable on this driver |
| `needs_external_setup` | optional | The value if present, plus `message` saying what to do on the PC |
| `unsupported` | absent | Greyed out with "Not supported on this GPU or driver", or hidden |
| `temporarily_unavailable` | absent | Greyed out with `message`. The client re-reads later |

- The top-level `state` applies the same five states to the whole panel. For example, `temporarily_unavailable` with `reason: "adlx_unavailable"` means ADLX failed to load, and `gpus` is then empty.
- A setting appears as `configurable` or `read_only` only if the host's probe of the installed driver proved it (see the [hardware matrix](hardware-matrix.md)). Settings from the catalogue that the probe didn't cover are `unsupported`. The catalogue ids are `<group>.<field>`, for example `anti_lag.enabled`, `chill.enabled`, `chill.min_fps`, `chill.max_fps`, `boost.enabled`, `boost.min_resolution`, `image_sharpening.enabled`, `image_sharpening.sharpness`, `enhanced_sync.enabled`, `frame_rate_target.enabled`, `frame_rate_target.fps`, `afmf.enabled`, `rsr.enabled`, `rsr.sharpness`, `vsync.mode`. Clients render ids they don't know generically from `kind`, `range`, `options` and `unit`.
- `kind` is `toggle` (bool value), `integer` (with `range`) or `choice` (string value, with `options: [{"value": "...", "label": "..."}]`).
- `gpus[].id` is the same `id` that `/api/hardware-metrics` reports. v1 lists the GPU the host encodes on.
- `host_control` is the caller's flag. `writable_now` is true only when the caller has host control and owns a `streaming` or `suspended` session. A GPU's `id` is the signed ADLX id that `/api/hardware-metrics` reports, so it can be negative; a write naming an id the host does not list is `not_found`. Reads go through the host's ADLX worker thread (the `telemetry.rs` pattern), are cached for 1 s, and never run on a stream thread.

**Writes.**

```http
POST /api/native/v1/radeon/settings
{ "gpu": 11520, "changes": [ { "id": "chill.enabled", "value": true }, { "id": "chill.max_fps", "value": 144 } ] }
```

1. The host validates the whole request first: every id exists and is `configurable`, every value has the right type and lies within its range and step. Otherwise it changes nothing and answers `invalid_value` or `not_writable`, naming the setting.
2. Before a setting changes for the first time in a session, the host writes its original value to the journal on disk and syncs the file. Later writes in the same session keep that first original.
3. The host applies the changes in order. If the driver refuses one, the host restores the ones from this request it has already applied and answers `driver_rejected` with that setting's id.
4. On success the host re-reads the settings and returns the full Radeon object. The driver's read-back is the truth, so dependent settings the driver changed show up too.

**Restore on session end.** Writes are global to the driver, and the journal ties them to the session:

- The host restores at End session, Close game, grace expiry, unpair or disable of the owner, host shutdown, on `POST /radeon/restore`, and at the next host start if a journal survived a crash. It does **not** restore at Disconnect: a suspended session keeps its settings through a resume, and they are restored when the session ends.
- A journaled setting is restored only if its current value still equals the value the host wrote. If someone changed it on the PC in the meantime, the local change wins, and the host drops the entry and logs it.
- A restore that fails is loud: it goes into the host log with the setting id, the entry stays in the journal, the host retries at the next start, and `GET /radeon` keeps showing `restore` with `message`.
- While a change is pending, `restore: { "original", "written" }` is present, so the overlay can say "Goes back to 120 fps when you end the session".

## Wake targets (`wake`)

Status: served since 3.0.0 ([design](wake-and-audio-design.md) A.2, A.3 and A.6). Types in `rubylight_native_api::wake`, fixture `rust/core/native-api/tests/fixtures/wake.json`.

```http
GET /api/native/v1/wake
```

```json
{
  "revision": 1,
  "primary": "02:7C:16:B4:13:4F",
  "targets": [
    {
      "mac": "02:7C:16:B4:13:4F",
      "kind": "ethernet",
      "connected": true,
      "route": true,
      "broadcasts": ["192.168.4.255"],
      "state": "ready",
      "checks": [
        { "id": "wake_enabled", "state": "ok" },
        { "id": "magic_packet", "state": "ok" },
        { "id": "magic_packet_only", "state": "warning" }
      ]
    }
  ],
  "system": {
    "sleep": "s3",
    "modern_standby": false,
    "hibernate": false,
    "fast_startup": "inactive",
    "shutdown_wake": "firmware_unknown"
  },
  "idle_sleep": { "state": "off", "minutes": 0, "reason": null }
}
```

- Any paired device with List, View or Launch can read it. Paired clients already see `<mac>` in `/serverinfo`; this adds the PC's other physical adapters. Virtual and VPN adapters are never listed.
- `primary` is the MAC `/serverinfo` reports to this client, in the same `aa:bb:cc:dd:ee:ff` format, chosen by the host's MAC policy. It is `null` when no physical adapter is connected (`/serverinfo` then sends zeros).
- `targets` lists every physical adapter, connected or not. `kind` is `ethernet` or `wifi`. `route` is true for the adapter this request came in through; every `route` is false when the client came through a VPN or another virtual adapter, which a sleeping PC can't be woken through without a relay on its LAN. `broadcasts` are the adapter's IPv4 subnet-directed broadcasts.
- `targets[].state`: `ready`, `needs_setup` (a wake setting is off; an administrator's `POST /api/power/wake/fix` from the console changes it), `unknown` (the settings could not be read, for example because PowerShell is blocked) or `experimental` (Wi-Fi).
- `checks[].id`: `wake_enabled` (Allow this device to wake the computer), `magic_packet` (`*WakeOnMagicPacket`), `magic_packet_only`, `wake_on_pattern` (information only), `modern_standby_magic_packet` (Modern Standby PCs whose adapter has it) and `shutdown_wake` (a vendor's wake from shutdown, information only). `checks[].state`: `ok`, `warning`, `error` or `unknown`.
- `system.sleep` is `s3` or `none`. `fast_startup` is `active` (on, with hibernate available), `inactive` (on, but hibernate is off, so it has no effect) or `off`. `shutdown_wake` is always `firmware_unknown`: waking from shutdown depends on firmware settings Windows can't read.
- `idle_sleep` is the host's own sleep after idle: `state` is `off`, `armed`, `blocked` or `unsupported`, `minutes` is `sleep_after_idle_minutes` (0 when off), and `reason` says what blocks it or why it is unsupported (`null` otherwise).
- Clients send to every target's MAC and every `broadcasts` address, on top of the serverinfo MAC and their own interfaces' broadcasts. Unknown fields are ignored, and unknown `kind`, `state`, `sleep`, `fast_startup` and `shutdown_wake` values decode as `Unknown`. A client shows a check id it doesn't know by its `state`.
- The endpoint only reads. NIC settings are changed only with consent, from the installer or the admin console, never through this API.

## Audio processing (`audio.night_mode`, `audio.headphone_surround`)

Status: served (3.0.0, [design](wake-and-audio-design.md) B.3, B.4 and B.6); Nightjar's stream menus show both switches. Types in `rubylight_native_api::audio`, fixtures `audio.json` and `audio_write.json`.

```http
GET /api/native/v1/audio
```

```json
{
  "revision": 3,
  "settings": [
    { "id": "audio.night_mode", "group": "sound", "kind": "toggle",
      "state": "configurable", "value": true },
    { "id": "audio.headphone_surround", "group": "sound", "kind": "toggle",
      "state": "configurable", "value": true,
      "reason": "next_stream", "message": "Headphone surround applies from the next stream." }
  ]
}
```

- The settings are Radeon `Setting` objects, `kind: toggle`, `group: sound`, with the ids `audio.night_mode` and `audio.headphone_surround`. Each appears only when the host lists its feature.
- `value` is the caller's session choice, resolved highest first: a live change in this session, the session's launch parameter, the app, the device, then the host. `revision` goes up with every change.
- A `reason` with `message` (plain English) is a note on a setting this stream doesn't apply. The owner can still write it, so it stays `state: configurable` with `value`, the choice. Only a setting the caller can't write is `temporarily_unavailable`, without a `value`: in 3.0 that is `no_session`. The reasons:

| `reason` | Meaning |
|---|---|
| `no_session` | The caller has no streaming or suspended session. The setting is `temporarily_unavailable` with no `value` |
| `client_surround` | The client asked for 5.1 or 7.1 and renders its own speakers (headphone surround only) |
| `game_only_audio` | Capture uses Game only audio, which is stereo (headphone surround only) |
| `stereo_endpoint` | The PC's playback device is stereo (headphone surround only) |
| `muted` | Audio isn't being streamed |
| `next_stream` | Turned on mid-stream without 8-channel capture: it applies from the next stream. Clients show "Applies next stream" |

Clients show a reason they don't know by `message`.

```http
POST /api/native/v1/audio/settings
{ "changes": [ { "id": "audio.night_mode", "value": false }, { "id": "audio.headphone_surround", "value": true } ] }
```

- For a device with its own live Stream session (also a second device streaming beside the PC's main session), with View or Launch. Host control isn't needed: the change affects only the caller's own stream. A Remote Monitor or input-only session has no settings here; GET reports them `temporarily_unavailable` (`no_session`) as without a stream.
- It applies to this session from the next audio packet and isn't saved. It answers the new Audio object.
- Refusals: `no_session` when the caller has no live Stream session of its own (none, a suspended one, only another device's, or only a monitor or input session); `not_found` for an id the host doesn't list; `invalid_value` for a value that isn't a boolean; `invalid_request` for unknown fields.

**Launch parameters.** `/launch` and `/resume` take `audioNightMode=on|off` and `audioHeadphoneSurround=on|off`, the session choice above. A client sends them only when the host lists the feature and the user picked something other than the host setting. Other values are logged and ignored, and hosts without the features ignore the parameters.

## Files

File transfer lets a paired device send files to the PC and fetch them back. It needs the host's `file_transfer` setting (off by default) and the device's Upload files or Download files bit, which no device has until the console grants it. Both are read on every request and every 250 ms while a transfer runs: turning either off ends a running upload with `403` and deletes its `.part` file, and ends a running download before its last byte. Files go to and come from `file_transfer_dir`, or the signed-in user's `Downloads\Rubylight` when that is empty, and are read and written as that Windows user.

While a stream runs or starts, uploads and downloads together move at `file_transfer_stream_mbps` (50 Mbit/s by default; 0 for full speed); the rate is read again every 250 ms. Otherwise they run at full speed.

```http
GET /api/native/v1/files
```

```json
{
  "folder": "C:\\Users\\Player\\Downloads\\Rubylight",
  "free_bytes": 107374182400,
  "max_file_bytes": 4294967296,
  "files": [ { "name": "clip (1).mp4", "size": 52428800, "modified_unix_ms": 1792000000000 } ]
}
```

Only plain files are listed: no folders, links or `.part` files. `free_bytes` is what an upload may use: the volume's free space less a reserve of 2 % of the volume, at least 1 GiB and at most 4 GiB, and less what the uploads running still have to write. A device with Upload files but not Download files gets `folder`, `free_bytes` and `max_file_bytes` with an empty `files`.

```http
POST /api/native/v1/files?name=clip.mp4
Content-Type: application/octet-stream
Content-Length: 52428800
```

- `name` is percent-encoded UTF-8 as RFC 3986 has it (`+` is a plus; a space is `%20`): a file name without a path, at most 200 UTF-16 units, with nothing Windows refuses (`\ / : * ? " < > |`, control characters, a trailing dot or space, device names such as `CON`). Anything else is `invalid_name`, and so is a name ending in `.part`.
- `Content-Length` is required (`length_required`), and checked against `max_file_bytes` (`file_transfer_max_mb`, 4096 MiB by default, `too_large`) and `free_bytes` (`insufficient_storage`) before a byte is written. The 1 MiB limit of the other HTTPS routes does not apply.
- The body goes to `<name>.part` and is renamed only once all of it arrived. A taken name gets the next free number, as Explorer numbers copies: `clip (1).mp4`. The answer is `201` with `{ "name": "clip (1).mp4", "size": 52428800 }`, where `size` is the bytes received.
- A body that ends early, sends nothing for 30 s, or after its first 30 s averages less than 64 KiB/s (time spent waiting for the stream rate doesn't count) is `incomplete_upload`, and the `.part` file is deleted. A longer body than its `Content-Length` is `invalid_request`.
- One upload per device at a time: a second one is `transfer_busy`. Other devices are not held up.
- A refused upload is answered at once with `Connection: close`. For a paired device the host then reads and drops up to 8 MiB or 5 s of the rest of the body before it closes, so a client that sends a small body before reading gets the answer, not a reset. A caller that isn't a paired device gets no such grace: the connection closes right after the answer.

```http
GET /api/native/v1/files/content?name=clip%20(1).mp4
```

The answer is `application/octet-stream` with the file's exact `Content-Length`. If the file grows or shrinks while it is sent, or the client takes nothing for 30 s, the host ends the connection before the last byte, so the client never keeps a short or mixed file. A name the folder doesn't hold as a plain file is `not_found`. A device runs at most two downloads at once and the PC eight; another is `transfer_busy`, as is a file another program on the PC holds open without sharing it.

## Image clipboard

The image clipboard is `type=png` on Vibepollo's `/actions/clipboard`, beside `type=text` and with its rules: Read the clipboard for `GET`, Write to the clipboard for `POST`, View or Launch, a session of this device and no background stream. The host lists the feature `clipboard.image` while its `clipboard_image` setting is on, which it is by default; while it is off, `type=png` is 403 like any type but `text`. The route is outside `/api/native/v1`: like its text form it answers with the status alone, so no answer carries an error body or a `code`.

- `GET /actions/clipboard?type=png` answers `image/png`, or 404 while the clipboard holds no image. A `PNG` another program put on the clipboard is passed on as it is; a bitmap (`CF_DIBV5` or `CF_DIB`, 24 or 32 bits a pixel) is converted. An image larger than the limit below is 413, one the host can't convert 415.
- `POST /actions/clipboard?type=png` takes `Content-Type: image/png` (415 without it) of at most `clipboard_image_max_mb` (16 MiB by default, 1 to 64), and is 413 above that: by its `Content-Length` before a byte is read, or once the body passes the limit. The 1 MiB limit of the other routes, text on this one included, doesn't apply to it. The host checks the image's size before it decodes the pixels (at most 16384 a side and 32 megapixels, else 413), then puts them on the clipboard as `CF_DIBV5` and `PNG`, re-encoded, so no other chunk of the file reaches the PC's programs. A body that isn't a PNG is 400, one the host can't decode 415.
- Image requests take turns across devices, and a device's image request made while another of its own is waiting or running is 429.

## Shared folders

Shared folders let a paired device browse folders of the PC, download from them and, with a `write` grant, upload to them. The host's `shared_folders` setting names the folders: a JSON list of `{"id": "games", "name": "Games", "path": "D:\\Games"}`, where `id` is 1 to 64 ASCII letters, digits, `-` or `_` and `path` is a folder on a drive of this PC (no network share, `\\?\` or `\\.\` path, `.`, `..` or stream name). The host lists the feature `folders` while it shares at least one; `folders.mount` (WebDAV) is not served.

A device sees a folder only with a grant for it: `"shared_folders": {"<folder id>": "read" | "write"}` in its `extra` map, set in the console. No `perm` bit or Full control grants a folder, and the File transfer setting isn't needed. `permissions.shared_folders` is `true` when the device holds at least one grant; it is missing from older hosts and then `false`. `/api/clients/update` refuses any other grant value, and `null` clears them; a hand-edited grant it would refuse, such as `"../x"`, grants nothing. Taking a grant away, or a `write` grant back to `read`, disconnects the device, as removing `perm` bits does. The setting and the grant are read on every request and every 250 ms while a transfer runs: dropping either, or pointing the folder's id at another path, ends a running upload with `403` and deletes its `.part` file, and ends a running download before its last byte.

| Method | Path | Needs | Result |
|---|---|---|---|
| GET | `/folders` | a grant for a shared folder | FolderList: the device's grants only, in the host's order |
| GET | `/folders/{id}/entries?path=` | a grant for `id` | FolderEntries |
| GET | `/folders/{id}/content?path=` | a grant for `id` | The file's bytes |
| PUT | `/folders/{id}/content?path=` | a `write` grant for `id` | `201` with `{ "name": ..., "size": ... }`, as an upload to `/files` |

```json
{ "folders": [ { "id": "games", "name": "Games", "access": "read" } ] }
{ "path": "saves", "entries": [ { "name": "slot1.sav", "kind": "file", "size": 4096, "modified_unix_ms": 1792000000000 } ] }
```

- `path` is percent-encoded UTF-8, as `name` is for `/files`: names below the folder separated by `/`, missing or empty for the folder itself. Each name follows the `/files` rules (up to 255 UTF-16 units here, and no `.part` name), and a path has at most 64. An empty name (a leading, trailing or double `/`), `.`, `..`, `\`, a stream name (`:`), a device name such as `CON`, or more than one `path` is `invalid_name`.
- Files are opened as the signed-in Windows user, only below the folder. Each name is checked against what Windows opened, so a link, junction or other reparse point (even one back into the folder) or an 8.3 short name is `not_found`, like anything missing; names match in any case, as Windows matches them. A shared folder may itself be reached through a junction.
- `entries` lists folders first, then plain files, each by name. Links, `.part` files and names a path can't hold are skipped. `kind` is `file` or `dir`; an unknown `access` or `kind` decodes as `Unknown`. `size` is 0 for a folder.
- `content` is sent as `GET /files/content` sends it: exact `Content-Length`, ended early if the file changes, the download caps and `transfer_busy`.
- A PUT's last name is the file, written into its folder, which must exist (`not_found`). It follows `POST /files`: a `.part` name is `invalid_name`; `length_required`, `too_large` (`file_transfer_max_mb`), `insufficient_storage`, `incomplete_upload` and `transfer_busy` (one upload per device, shared with `/files`); the `.part` file renamed once all of it arrived; a taken name numbered; a refused body answered with `Connection: close`.
- Uploads and downloads count against file transfer's caps and move at `file_transfer_stream_mbps` while a stream runs, together with `/files`.

| HTTP | `code` | When |
|---|---|---|
| 403 | `folders_disabled` | The host shares no folders |
| 403 | `forbidden` | The device has no grant for the folder (or for any, on `GET /folders`), or only `read` for a PUT |
| 403 | `folder_unreadable` | Windows denies the signed-in user reading the folder, or a folder or file in it (`entries`, `content`) |
| 404 | `not_found` | The host no longer shares the granted folder, or the path isn't in it |

## The host control permission

- It is stored per device as `"host_control": true` in that client's `extra` map in the paired state, like `allow_client_commands`. The host reads `true` and `"true"` as on, like the other boolean keys in `extra`; a missing key or anything else means off. That covers new pairings, migrated Vibepollo or Apollo state and old backups, so the flag is off by default.
- Why not a `perm` bit: bits 24 to 31 are the Apollo-family action group, `Permission` is sent to Artemis, `/api/clients/update` masks `perm` with `0x071f1f00`, and test fixtures and imports use `perm = u32::MAX`. A separate key can't collide with an upstream bit, and an all-ones permission value can't switch it on.
- Only the admin console can grant it: Devices → device → Other → "Host control: change Radeon settings and close games from this device". `/api/clients/update` accepts only `true`, `false` or `null` for it. Nothing in the pairing flow or on the streaming port can set it.
- It is required for every `POST /radeon/*` and for `close_game`. Reads never need it.
- Revoking it takes effect on the next request and doesn't stop the stream (unlike removing `perm` bits). Pending Radeon changes are still restored at session end.

## USB forwarding

Feature `usb` is listed while the host's USB forwarding setting (`usb_forwarding`, off by default) is on and its USB/IP driver (usbip-win2) is installed. The device needs the `usb` permission.

`usb` is a boolean in `permissions`, missing from older hosts and then `false`. Like host control, it is not a `perm` bit, and only the admin console sets it. Unlike host control, turning `allow_usb` off disconnects the device, as removing `perm` bits does. It is `"allow_usb": true` in the device's `extra` map: only a JSON `true`, never a string, a number, a `perm` bit or Full control. `/api/clients/update` accepts `true`, `false` or `null` for it.

| Method | Path | Needs | Result |
|---|---|---|---|
| GET | `/usb/tunnel` | `usb`, a token | 101, then USB/IP |

- The USB tunnel is a request with `Connection: Upgrade`, `Upgrade: rubylight-usbip/1` and `X-Rubylight-Usb-Token` (32 hex digits). The token comes from control message `0x553A` ([control messages](control-messages.md#0x553a-usb-forwarding)): it is good once, for 30 s, and only for the device that asked for it. After `101 Switching Protocols` the connection carries standard USB/IP, with the client as the USB/IP server. The host relays it to its driver over 127.0.0.1, checking every message against what the driver asked, so no USB/IP port is reachable from the network and a client can't hand the driver malformed USB/IP. A missing token or `Upgrade` is `invalid_request`, and an unknown, expired, already used or another device's token `not_found`.
- An attachment ends with the client's own DETACH, the console's detach (`POST /api/usb/detach`), either end closing the tunnel, the stream ending, or the device disconnecting. The client hears why on `0x553A`, unless it is the one that disconnected.

| HTTP | `code` | When |
|---|---|---|
| 403 | `usb_disabled` | The host's USB forwarding setting is off, or the device has no `usb` permission |
| 409 | `usb_limit` | The host already forwards as many devices as it allows |

## Reserved features

These are defined in `rubylight-native-api` but no host serves them yet, so none lists their features. Until one does, the routes below answer 404 `not_found` like any unknown path under `/api/native/v1`.

| Feature | Listed while | Permission |
| --- | --- | --- |
| `folders.mount` | as `folders`, for WebDAV | `shared_folders` |

| Method | Path | Needs | Result |
|---|---|---|---|
| any | `/folders/{id}/dav/` | a grant for `id` | WebDAV class 1 |

## Errors

Every non-2xx answer has the same body:

```json
{ "error": { "code": "host_control_required",
             "message": "Turn on Host control for this device in the Rubylight console.",
             "setting": null, "retry_after_ms": null } }
```

| HTTP | `code` | When |
|---|---|---|
| 403 | `not_paired` | No TLS client certificate, or the device is unknown or disabled |
| 403 | `permission_required` | List, View or Launch missing. `message` names the permission, as `authenticated()` does for the Moonlight endpoints |
| 403 | `host_control_required` | Write or Close game without host control |
| 403 | `file_transfer_disabled` / `forbidden` | Files: the host's File transfer setting is off / the device lacks Upload files or Download files |
| 404 | `not_found` | Unknown path under `/api/native/v1`, GPU or setting id, or Radeon controls this host does not have (a restore or write on a host without them) |
| 400 | `invalid_request` | Malformed JSON, unknown fields in a write, or a body over 16 KiB |
| 422 | `invalid_value` / `not_writable` | Wrong type, out of range, or the setting is not `configurable` |
| 409 | `no_session` / `not_session_owner` / `session_busy` | A write or action needs a session the caller owns. `session_busy`: a launch or teardown is in progress, retry after `retry_after_ms` |
| 409 | `driver_rejected` | ADLX refused a write. `setting` names it, and the earlier changes of the request were rolled back |
| 400 | `invalid_name` / `incomplete_upload` | Files: a name the host refuses / an upload that ended or stalled before its `Content-Length` |
| 411 | `length_required` | An upload without `Content-Length` |
| 413 | `too_large` | An upload over `max_file_bytes` |
| 507 | `insufficient_storage` | The PC has no room for the upload |
| 409 | `no_console_user` / `transfer_busy` | Files: nobody is signed in to Windows / the device already has an upload or two downloads running, the PC eight downloads, or another program holds the file |
| 503 | `temporarily_unavailable` | ADLX is busy or reinitialising, with `retry_after_ms` |
| 500 | `internal` | A bug. The message is logged on the host too |

`not_streaming` appears only as an action `reason`: a `disconnect` POST on a session that is already `suspended` is idempotent and returns 200 (with no session at all it is `409 no_session`). `message` is plain English that the client can show as it is. Clients branch on `code` only. An unknown code decodes as `ErrorCode::Unknown`, which carries no status, so the client must keep the HTTP status of the response and treat the error by that.

Which actions are allowed (`GET /session`, and the same rules apply to the POSTs): `resume` for the owner while `suspended` or `streaming` (`/resume` supersedes the same device's stream, which is the reconnect after a crash); `disconnect` while `streaming`; `end` in any state but `idle`; `close_game` as `end`, or with no session when a game is left running. While `preparing` or `launching`, `resume`, `end` and `close_game` are refused with `session_busy`, because the launch holds `launch_transition` for as long as it takes. In `stopping`, `end` and `close_game` stay allowed (repeating is safe) and `resume` is refused with `session_busy`.

## Versioning and compatibility

- The path carries the major version (`/v1`). Within v1, changes are additive only: new fields, features, setting ids, actions, error codes and enum values. Clients ignore unknown fields. They treat an unknown state as `unsupported`, an unknown session state as `idle` with no actions, and hide unknown actions.
- Removing or renaming a field, changing a type, or changing an action's meaning needs `/v2`. Then `NativeApiVersion` reports 2, and the host keeps serving v1 for at least the whole 3.x line.
- `api_version` in the capabilities response is the version of the path that answered.
- Compatibility: stock Moonlight and Nightjar 0.7.x ignore the field on a 3.0 host, and `/applist`, `/cancel` and the tiles are unchanged for them; Nightjar 0.8.0 and newer against 2.6.x, Sunshine or Apollo finds the field missing and shows the classic grid.

## Where the types live

The API types live in a dependency-light crate inside `rust/core`, re-exported by butterpollo-core:

- `rust/core/native-api/Cargo.toml`: package `rubylight-native-api`, a workspace member. Its only dependency is `serde` with `derive` (`serde_json` for tests). Its manifest is self-contained: it sets `version`, `edition = "2021"`, `rust-version = "1.80"`, `license` and `repository` itself and copies the workspace lints, because a `*.workspace = true` key makes cargo read the root manifest, whose `resolver = "3"` a 1.80 cargo can't parse. The crate has its own version, `1.0.0`, which follows the API and not the host, so `rust/release/bump.py` (which only bumps `rust/*/Cargo.toml`) leaves it alone.
- `butterpollo-core` depends on it by path and exposes it as `butterpollo_core::native_api` (`pub use rubylight_native_api as native_api;` in `rust/core/src/lib.rs`).
- Why not `butterpollo-core` itself: nightjar-core uses butterpollo-core only as a **dev-dependency** for interop tests. As a runtime dependency it would pull `x509-parser`, `rcgen`, `regex`, `quick-xml`, `prost`, `anyhow` and an `embed-resource` build script into the Android library, and its edition 2024 / Rust 1.88 floor into Nightjar's 1.80 workspace. Why not `rubylight-protocol`: it is `no_std` without serde, and JSON doesn't belong there.

Modules (serde shapes as in the examples above, all `snake_case`):

```rust
// rubylight_native_api
pub const API_VERSION: u32 = 1;
pub const PREFIX: &str = "/api/native/v1";
pub const SERVERINFO_FIELD: &str = "NativeApiVersion";
pub mod feature { pub const LIBRARY: &str = "library"; pub const SESSION: &str = "session";
    pub const CLOSE_GAME: &str = "session.close_game"; pub const RADEON_READ: &str = "radeon.read";
    pub const RADEON_WRITE: &str = "radeon.write"; }

// capabilities.rs
pub struct Capabilities { pub api_version: u32, pub host: HostInfo, pub features: Vec<String>, pub permissions: Permissions }
pub struct HostInfo { pub name: String, pub version: String, pub unique_id: String }
pub struct Permissions { pub list: bool, pub view: bool, pub launch: bool, pub host_control: bool }

// library.rs
pub struct Library { pub revision: String, pub games: Vec<Game> }
pub struct Game { pub install_id: String, pub app_id: u32, pub title: String, pub game_id: Option<String>,
    pub store: Option<Store>, pub installed: Option<bool>, pub running: bool, pub artwork: Artwork }
pub struct Store { pub source: String, pub id: String, pub name: String }
pub struct Artwork { pub cover: Option<String>, pub version: String }

// session.rs
pub enum State { Idle, Preparing, Launching, Streaming, Suspended, Stopping, #[serde(other)] Unknown }
pub enum Owner { None, You, OtherDevice, #[serde(other)] Unknown }
pub enum ActionId { Resume, Disconnect, End, CloseGame, #[serde(other)] Unknown }
pub struct Action { pub id: ActionId, pub allowed: bool, pub reason: Option<ErrorCode> }
pub struct SessionApp { pub install_id: String, pub app_id: u32, pub title: String }
pub struct Session { pub revision: u64, pub state: State, pub owner: Owner, pub app: Option<SessionApp>,
    pub game_running: bool, pub since_unix_ms: Option<u64>, pub suspended_until_unix_ms: Option<u64>,
    pub actions: Vec<Action> }

// radeon.rs
pub enum ControlState { Configurable, ReadOnly, NeedsExternalSetup, Unsupported, TemporarilyUnavailable,
    #[serde(other)] Unknown }
pub enum Kind { Toggle, Integer, Choice, #[serde(other)] Unknown }
#[serde(untagged)] pub enum Value { Bool(bool), Integer(i64), Choice(String) }
pub struct Range { pub min: i64, pub max: i64, pub step: i64 }
pub struct Choice { pub value: String, pub label: String }
pub struct Restore { pub original: Value, pub written: Value }
pub struct Setting { pub id: String, pub group: String, pub kind: Kind, pub state: ControlState,
    pub value: Option<Value>, pub range: Option<Range>, pub options: Vec<Choice>, pub unit: Option<String>,
    pub restore: Option<Restore>, pub reason: Option<String>, pub message: Option<String> }
pub struct Gpu { pub id: i32, pub name: String, pub driver: String, pub settings: Vec<Setting> }
pub struct Radeon { pub revision: u64, pub state: ControlState, pub reason: Option<String>,
    pub message: Option<String>, pub host_control: bool, pub writable_now: bool, pub gpus: Vec<Gpu> }
pub struct SettingsWrite { pub gpu: i32, pub changes: Vec<Change> }   // deny_unknown_fields
pub struct Change { pub id: String, pub value: Value }

// wake.rs (feature::WAKE), check-id constants in wake::check
pub enum AdapterKind { Ethernet, Wifi, #[serde(other)] Unknown }
pub enum TargetState { Ready, NeedsSetup, Experimental, #[serde(other)] Unknown }
pub enum CheckState { Ok, Warning, Error, #[serde(other)] Unknown }
pub enum Sleep { S3, None, #[serde(other)] Unknown }
pub enum FastStartup { Active, Inactive, Off, #[serde(other)] Unknown }
pub enum ShutdownWake { FirmwareUnknown, #[serde(other)] Unknown }
pub enum IdleSleepState { Off, Armed, Blocked, Unsupported, #[serde(other)] Unknown }
pub struct Check { pub id: String, pub state: CheckState }
pub struct Target { pub mac: String, pub kind: AdapterKind, pub connected: bool, pub route: bool,
    pub broadcasts: Vec<String>, pub state: TargetState, pub checks: Vec<Check> }
pub struct System { pub sleep: Sleep, pub modern_standby: bool, pub hibernate: bool,
    pub fast_startup: FastStartup, pub shutdown_wake: ShutdownWake }
pub struct IdleSleep { pub state: IdleSleepState, pub minutes: u32, pub reason: Option<String> }
pub struct Wake { pub revision: u64, pub primary: Option<String>, pub targets: Vec<Target>,
    pub system: System, pub idle_sleep: IdleSleep }

// audio.rs (feature::AUDIO_NIGHT_MODE, feature::AUDIO_HEADPHONE_SURROUND)
pub const NIGHT_MODE: &str = "audio.night_mode"; pub const HEADPHONE_SURROUND: &str = "audio.headphone_surround";
pub const GROUP: &str = "sound"; // reason constants in audio::reason
pub struct Audio { pub revision: u64, pub settings: Vec<radeon::Setting> }
pub struct AudioWrite { pub changes: Vec<radeon::Change> }   // deny_unknown_fields

// error.rs
pub enum ErrorCode { NotPaired, PermissionRequired, HostControlRequired, NotFound, InvalidRequest,
    InvalidValue, NotWritable, NoSession, NotSessionOwner, SessionBusy, DriverRejected,
    TemporarilyUnavailable, Internal, NotStreaming, #[serde(other)] Unknown }
pub struct ErrorBody { pub error: ApiError }
pub struct ApiError { pub code: ErrorCode, pub message: String, pub setting: Option<String>,
    pub retry_after_ms: Option<u64> }
impl ErrorCode { pub fn status(self) -> u16 { /* table above */ } }
```

`Option` fields skip serialisation when `None`, except where the examples show `null` on purpose (`game_id`, `store`, `installed`, `app`, `setting` and `retry_after_ms` in an error, and `primary` and `idle_sleep.reason` in Wake). Response structs don't deny unknown fields, so old clients read newer hosts. In a Radeon response, a setting's `value`, `range` or `restore` of a shape the client doesn't know (a float, an array) reads as absent instead of failing the whole response. Requests stay strict. Request structs do, so typos fail loudly. The crate carries JSON fixtures (`rust/core/native-api/tests/fixtures/*.json`, one per example above) with round-trip tests, and Nightjar's tests reuse them.

## Design decisions

1. **A running game with no session.** The session machine has six states and no extra one for this case. After End session the game keeps running, the host reports `idle` with `game_running: true`, Resume becomes a new `/launch` or `/resume` of that game, and Close game works from there.
2. **Radeon writes need an owned session.** v1 allows writes only for the owner of a `streaming` or `suspended` session, because the restore is tied to the end of that session. Per-game Radeon profiles cover settings chosen before launch.
3. **One GPU.** v1 lists only the GPU that encodes the stream.
4. **Added games only.** v1 lists the games in the host's library; installed games that have not been added are not listed.
5. **Polling.** Clients poll `/session` and compare `revision`; there is no long poll.
6. **Covers only.** Artwork is the cover image; hero and logo images are not served.
7. **Session actions replace Quit.** A client that sees `NativeApiVersion >= 1` offers the native session actions instead of Moonlight's "Quit app". The host keeps serving `/cancel` unchanged for stock Moonlight.
8. **A separate types crate.** The types live in `rubylight-native-api` rather than `butterpollo-core` or `rubylight-protocol`, for the reasons in [Where the types live](#where-the-types-live).
9. **Probed settings.** The host's probe of the installed driver decides which Radeon settings are listed and their ranges.
