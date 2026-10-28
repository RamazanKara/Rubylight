# HTTP API

[Docs](README.md) · [Getting started](getting-started.md) · [Configuration](configuration.md)

Rubylight's Windows Rust host serves its administration API at `https://localhost:47990/api`. The console uses the same API. Changing the base `port` setting moves the console to `port + 1`.

This guide covers the current Rust implementation: routing, authentication and token scopes in [web.rs](../rust/host/src/web.rs), and the handlers for each area in [web/](../rust/host/src/web). Moonlight pairing, app discovery and streaming use separate protocol endpoints in [nvhttp.rs](../rust/host/src/nvhttp.rs). The [historical C++ API guide](https://github.com/RamazanKara/Rubylight/blob/2.0.0-rc.23/docs/legacy/api-cpp.md) is kept at the 2.0.0-rc.23 tag as a migration reference.

Paired clients use a separate API on the streaming HTTPS port: [Native API v1](native-api.md) (`/api/native/v1`, new in 3.0) covers the game library, session state and actions, and Radeon settings. It authenticates with the client certificate it paired with, so none of the methods below apply to it.

For client overlay telemetry, see [Hardware metrics](hardware-metrics.md). Its `/api/hardware-metrics` endpoint uses the streaming HTTPS port and the paired client's TLS certificate; the administration authentication methods below do not apply to it.

[Remote connection route](network-path.md) documents the separately authenticated `/api/network-path` endpoint and `NetworkPathVersion` capability. It returns only the requesting peer's current Tailscale route category.

## Authentication

Create the administrator account through the local console first. Initial account creation is restricted to requests from the host PC. Access to the web interface also follows `origin_web_ui_allowed`: `pc`, `lan` (the default), or `wan`.

| Method | Use |
| --- | --- |
| HTTP Basic | Send the administrator username and password over HTTPS. Useful for interactive scripts. |
| Scoped API token | Send `Authorization: Bearer <token>`. Each token grants only the selected path and HTTP methods. |
| Web session | Sign in with `POST /api/auth/login`. The host issues secure, HttpOnly session and refresh cookies; it also returns the access, refresh and CSRF tokens in JSON. |

The public API routes are `/api/auth/status`, `/api/auth/login`, `/api/auth/refresh`, `/api/csrf-token` and `/api/configLocale`. Other API routes require authentication, apart from local first-time account creation.

### Scoped tokens

Use the console's API token controls, or these administrator-authenticated routes. **Select all permissions** selects every available path and method, including paths hidden by search. You can clear the selection or deselect individual permissions before creating the token. The host accepts its full advertised catalog; tokens are no longer limited to 64 paths.

| Method and path | Result |
| --- | --- |
| `GET /api/token/routes` | Available scope paths and their allowed methods. |
| `POST /api/token` | Create a token from a `scopes` array; returns its secret once. |
| `GET /api/tokens` | Token hashes, ownership, creation times and scopes. |
| `DELETE /api/token/{hash}` | Revoke a token. |

A read-only token request body can be:

```json
{
  "scopes": [
    { "path": "/api/metadata", "methods": ["GET"] },
    { "path": "/api/session/status", "methods": ["GET"] }
  ]
}
```

Choose paths exactly as returned by `/api/token/routes`, including any pattern entries. The host validates the selected methods and matches each scope against the whole request path. Token-management routes cannot themselves be granted as token scopes. Secrets are stored as hashes; keep the creation response if the token will be used later.

### Sessions and CSRF

Browser sign-in takes `username`, `password` and optional `remember_me`. `POST /api/auth/refresh` rotates the refresh token; `POST /api/auth/logout` revokes the current session. `GET /api/auth/sessions` lists sessions, and `DELETE /api/auth/sessions/{id}` revokes one. Remembered sessions survive host restarts.

For requests that change state:

- If an `Origin` header is present, it must match the host's HTTPS origin or an entry in `csrf_allowed_origins`. A CSRF token does not override a rejected origin.
- Session authentication, whether through a cookie or a Bearer access token, requires `X-CSRF-Token` on protected routes. Read the session's token from `GET /api/csrf-token` using the same session.
- Basic authentication and valid scoped API tokens do not require a session CSRF token. Origin checks still apply. A Basic-authenticated request without `Origin` is refused with `403` when it has a form content type (`application/x-www-form-urlencoded`, `multipart/form-data`, `text/plain`) or `Sec-Fetch-Site` other than `same-origin` or `none`: send JSON or no body.
- Before browser sign-in or first-time account creation, fetch `/api/csrf-token`, retain its anonymous cookie, and send the returned token as `X-CSRF-Token`. If that cookie is present, the host requires a matching header.

## Read-only examples

These PowerShell examples read host status without launching an app or changing settings. Replace the certificate path with the console's root from the active profile and `admin` with your administrator name. `curl.exe --user admin` prompts for the password instead of putting it in the command.

The console's certificate names `localhost`, `127.0.0.1` and `::1`. On an installed host, Windows' `curl.exe` also trusts it without `--cacert`, through the root setup adds to Windows, when given `--ssl-no-revoke`: the root publishes no revocation list, and Windows' curl otherwise refuses a certificate it cannot check for revocation. Browsers do not need this.

```powershell
$certificate = 'C:\path\to\profile\credentials\console-root.pem'
curl.exe --fail-with-body --silent --show-error --cacert $certificate --user admin https://localhost:47990/api/metadata
curl.exe --fail-with-body --silent --show-error --cacert $certificate --user admin https://localhost:47990/api/session/status
curl.exe --fail-with-body --silent --show-error --cacert $certificate --user admin https://localhost:47990/api/token/routes
```

`/api/metadata` (also served as `/api/meta`) reports the host version, codec probe status, configured capture backend, displays, audio endpoints and virtual-display capabilities. Results are cached for up to five seconds; reading them does not start a codec probe. Codec state is `checking`, `ready` or `failed`.

`/api/session/status` includes `activeSessions`, `appRunning`, `appName`, `paused` and `running`. An app can remain running after its stream disconnects, so these fields describe different states.

`GET /api/session/console` returns `recovery` with `session_id`, `console_session_id`, `can_reattach` and a human-readable `reason`. After explicit user confirmation, `POST /api/session/console` with those two IDs requests that Windows return the host's own RDP session to the physical console. The destination must have no signed-in user, or be disconnected with a verified matching user SID. Active sessions, different users and unknown identities are refused. The host rechecks session identity, destination eligibility and idle transports before making the request; it never selects another user's source session. The response acknowledges the request, not completion of the asynchronous Windows transition. The same authentication, token-scope and CSRF rules as other administration endpoints apply. This can disconnect RDP and make the desktop visible at the PC.

## Endpoint groups

The routes below are implemented by the Rust host. JSON requests use `Content-Type: application/json`; most responses include a Boolean `status`. Display lists, images and log downloads have their own response formats.

| Area | Read | Change |
| --- | --- | --- |
| Host settings | `GET /api/config`, `GET /api/metadata`, `GET /api/configLocale` | `POST` or `PATCH /api/config` |
| Applications | `GET /api/apps`, `GET /api/apps/{id}/cover`, `GET /api/apps/{uuid}/icon` | `POST /api/apps`, `DELETE /api/apps/{id}` (or `POST /api/apps/delete` with `{"uuid": ...}`), `POST /api/apps/reorder`, `POST /api/apps/launch`, `POST /api/apps/close` |
| Paired devices | `GET /api/clients/list`, `GET /api/clients/pending` | `POST /api/pin`, `POST /api/otp`, `POST /api/pairing/qr` (Experimental, with `qr_pairing` on), `POST /api/clients/update`, `POST /api/clients/disconnect`, `POST /api/clients/unpair`, `POST /api/clients/unpair-all`, `POST /api/clients/reset-network-history` |
| Webhooks ([format](webhooks.md)) | `GET /api/webhooks` | `POST /api/webhooks`, `POST /api/webhooks/delete`, `POST /api/webhooks/test` |
| Active sessions | `GET /api/session/status`, `GET /api/rtsp/sessions` | Use the application or device controls above. |
| Forwarded USB devices | `GET /api/usb` | `POST /api/usb/detach` |
| Displays | `GET /api/display-devices`, `GET /api/clients/display-layout`, `GET /api/clients/hdr-profiles`, `GET /api/display/golden_status` | `PUT /api/clients/display-layout`, `POST /api/display/export_golden`, `POST /api/display/restore_golden`, `DELETE /api/display/golden`, `POST /api/display/terminate_virtual`, `POST /api/reset-display-device-persistence` |
| Frame limiting and HDR | `GET /api/frame-limiter/status`, `GET /api/rtss/status`, `GET /api/health/vulkan-hdr-layer`, `GET /api/framegen/edid-refresh?device_id=...` | `POST /api/health/vulkan-hdr-layer/register` |
| Steam | `GET /api/steam/status`, `GET /api/steam/games` | `POST /api/steam/force_sync`, `POST /api/steam/launch` |
| Playnite | `GET /api/playnite/status`, `GET /api/playnite/games`, `GET /api/playnite/categories` | `POST /api/playnite/install`, `POST /api/playnite/uninstall`, `POST /api/playnite/force_sync`, `POST /api/playnite/cover`, `POST /api/playnite/launch` |
| Library extras | `GET /api/covers/{id}`, `GET /api/lossless_scaling/status`, `GET /api/browse?path=...&type=...` | `POST /api/covers/upload`, `POST /api/apps/purge_autosync` |
| Updates | `GET /api/updates` | `POST /api/updates/check`, `POST /api/updates/install`, `POST /api/updates/install_now`, `POST /api/updates/cancel` |
| Diagnostics | `GET /api/logs`, `GET /api/logs/tail`, `GET /api/logs/analysis`, `GET /api/logs/export`, `GET /api/logs/export_crash`, `GET /api/logs/export_crash/manifest`, `GET /api/health/crashdump` | `POST /api/health/crashdump/dismiss` |
| Administration | Authentication and token routes above. | `POST /api/password`, `POST /api/restart`, `POST /api/quit` |

Useful response and request details:

- `/api/clients/list` includes read-only `network_loss_routes` for each paired device: address, seconds until expiry, and eligible sessions until a fast-rate probe. Authenticated `POST /api/clients/reset-network-history` with `{"uuid": "device-id"}` clears those routes for subsequent streams; it does not change a running stream. Client updates cannot set this telemetry.
- `GET /api/usb` lists `devices` that clients forward: `id`, `client`, `name`, `vid`, `pid`, `class`, `speed`, `busid`, `state` (`waiting` for the tunnel, `opening`, `attached` or `detaching`), the driver's `port` once attached, and `since_unix_ms`. `POST /api/usb/detach` with `{"id": 1}` detaches one; the client is told.
- `/api/clients/list` also has a top-level `sleep_unavailable`: why this PC offers no Sleep PC, such as a PC with only Modern Standby, or `null` when it does.
- `POST /api/clients/update` takes `host_control` per device. Only `true`, `false` or `null` are accepted, and a missing key means off. Only the console path grants it ([host control](configuration.md#host-control-for-a-device)).
- `POST /api/clients/update` takes `allow_sleep` (Sleep PC) the same way: only `true`, `false` or `null`, and anything but a JSON `true` means off.
- Configuration writes merge submitted fields and return `restart_required: true`. Treat the response as a saved configuration, not confirmation that every active stream has adopted it.
- Applications are updated by their `uuid`; deletion accepts the UUID or numeric app ID. Read `/api/apps` before making changes rather than treating an array index as the identity.
- `/api/logs` and `/api/logs/export` return up to the last 8 MiB of the current log as text. `/api/logs/export_crash` creates the support ZIP. The ordinary log export is not a ZIP.
- `/api/apps/{uuid}/icon` returns the PNG icon a Playnite sync saved for the app, or `404`.
- `/api/browse` lists a folder of this PC for a file picker: `path`, `parent` and `entries` (each with `name`, `path` and `type` `directory` or `file`), folders first. `type=executable` lists only `.exe`, `.bat`, `.cmd` and `.ps1` files; no `type`, `type=any` or `type=file` any file; `type=directory` or any other value no files. Folders are always listed. A file or missing path lists the nearest existing folder above it; an empty `path` lists the drives, and a drive root's `parent` is empty.
- `/api/playnite/cover` takes `playnite_id` and `cover_key` (an image saved by `/api/covers/upload` or a cover search), asks the Playnite plugin to use that image as the game's cover, then syncs the library and returns the cover's `path`. `/api/playnite/launch` restarts Playnite in desktop mode.
- `/api/logs/analysis` reads the last 8 MiB of the current log on the host and returns `findings`, newest first, plus the `since` and `until` times it covered. Each finding has `id`, `category` (`encoder`, `capture`, `network` or `driver`), `level` (`warn` or `error`), `title`, `explanation`, `fix`, a `link` to the troubleshooting guide, `count`, `first_seen`, `last_seen` and the latest matching line as `sample`. It uses fixed local rules and contacts nothing.
- `/api/clients/list` has read-only `client_agent`, the `User-Agent` of the device's latest `/serverinfo` since the host started (or `null`), and `client_warning`, `{"problem", "link"}` for a client build with a known problem against this host, or `null`. The warning never refuses the device.
- `/api/logs/tail` accepts `offset` and `max`, then returns `text`, the next `offset`, `size` and a `reset` flag for log rotation.
- `/api/updates/install` queues the update for an idle installed service. `/api/updates/install_now` installs without waiting and ends a running stream; `now` is `true` in `/api/updates` while such an update is queued. Read `/api/updates` for progress and the result.

Authentication failures return HTTP `401`; denied origins or network reach return `403`; invalid requests commonly return `400` with `{"status":false,"error":"..."}`. Login rate limiting returns `429` for the sign-in endpoint and `401` for HTTP Basic authentication. Check the HTTP status and response body together.

For exact request fields beyond this overview, follow [the API router](../rust/host/src/web.rs) and [its area handlers](../rust/host/src/web), [token scope validation](../rust/core/src/auth.rs) and [administration checks](../rust/tests/web_api.py).

## Native API v1 (paired clients)

New in 3.0. Nightjar and other paired clients use `/api/native/v1` on the streaming HTTPS port (`HttpsPort` from `/serverinfo`, normally 47984). The administration methods above do not apply to it: no Basic auth, cookies or API tokens, and nothing on plain HTTP or the console port. [Native API v1](native-api.md) is the design and contract; this section is the short reference for using it. Where the two differ, the host code in [native_api.rs](../rust/host/src/native_api.rs) and [native_api/radeon.rs](../rust/host/src/native_api/radeon.rs) is right.

### Authentication

The client presents the TLS certificate it paired with. The host looks the device up on every request, so a change in the console applies to the next call.

- Not paired (no certificate, or an unknown, disabled or permission-less device): `403 not_paired`.
- The permissions are the usual List, View and Launch. A missing one is `403 permission_required`, and the message names it.
- **Host control** is a separate per-device flag, off by default, that only the console can turn on (Devices → device → Other). It is required for `close-game` and for every `POST /radeon/*`. Reads never need it. Turning it off applies to the next request and does not end a stream.
- Over plain HTTP, every `/api/native/v1` path is `404`: the API exists on HTTPS only.

### Discovery

`GET /serverinfo` has one more field, `NativeApiVersion`:

| Value | Meaning |
|---|---|
| missing | No native API (2.6, Sunshine, Apollo, Vibepollo): use the classic app grid |
| `0` | The host has it, but not for this request: plain HTTP, or a certificate that is not paired, is disabled or has no permissions (`perm` 0) |
| `1` | A paired HTTPS request may call `/api/native/v1/capabilities` |

Stock Moonlight ignores the field. Every other `/serverinfo` field, `/applist` and `/cancel` answer exactly as before, which the host tests pin byte for byte.

### Endpoints

| Method | Path | Needs | Answer |
|---|---|---|---|
| GET | `/capabilities` | paired | API version, host name, version and id, feature list, the caller's permissions |
| GET | `/library` | List | The added games with `install_id`, `game_id`, store, installed and running flags and cover URLs |
| GET | `/session` | paired | State, owner, app, allowed actions. Other devices see state and owner only |
| POST | `/session/disconnect` | View or Launch, session owner | Stops the stream and keeps the game and display for the grace period |
| POST | `/session/end` | Launch, session owner | Ends the stream and releases the display. The game keeps running |
| POST | `/session/close-game` | Launch and Host control | Like End session, and quits the game. Also works with no session when a game is left running |
| GET | `/radeon` | View or Launch | Radeon settings of the encoding GPU, each in one of five states |
| POST | `/radeon/settings` | View and Host control, owner of a `streaming` or `suspended` session | Applies `{ "gpu": id, "changes": [{ "id", "value" }] }` and answers with the new Radeon state |
| POST | `/radeon/restore` | View and Host control, owner of a `streaming` or `suspended` session | Puts back everything this session changed |

Playing and resuming stay on `/launch` and `/resume`. Cover images stay on `/appasset`. The API never carries video, input or telemetry.

All answers are `application/json; charset=utf-8` with `Cache-Control: no-store`. A POST body is empty or JSON of at most 16 KiB, sent as `application/json`, `text/plain` or without a content type; actions take no fields. Clients poll `/session` about every 2 s while a home or detail screen shows.

Session states are `idle`, `preparing`, `launching`, `streaming`, `suspended` and `stopping`. After End session the game can still run: the state is `idle` with `game_running: true`. A repeated action is safe and answers `200` with the new session; `end` and `close-game` wait up to 5 s for the streams to stop.

### Errors

Every non-2xx answer over HTTPS has one body shape. (Over plain HTTP the native paths do not exist, and the host's router answers a bare `404`.) Clients branch on `code` and keep the HTTP status:

```json
{ "error": { "code": "host_control_required", "message": "Turn on Host control for this device in the Rubylight console.", "setting": null, "retry_after_ms": null } }
```

| HTTP | `code` | When |
|---|---|---|
| 403 | `not_paired` | No certificate, or the device is unknown or disabled |
| 403 | `permission_required` | List, View or Launch is off |
| 403 | `host_control_required` | A write or Close game without Host control |
| 404 | `not_found` | Unknown path or method under the prefix, a GPU or setting id the host does not list. On a host without Radeon controls the POSTs are `404`, while `GET /radeon` answers `200` with state `unsupported` and reason `no_radeon` |
| 400 | `invalid_request` | Malformed JSON, unknown fields in a write, a content type other than JSON or text, or a body over 16 KiB |
| 422 | `invalid_value`, `not_writable` | Wrong type or range, or the setting is not `configurable`. `setting` names it |
| 409 | `no_session`, `not_session_owner`, `session_busy` | The caller owns no session, another device owns it, or a launch or teardown is in progress (`retry_after_ms` says when to retry) |
| 409 | `driver_rejected` | The driver refused a write. The earlier changes of the request were rolled back |
| 503 | `temporarily_unavailable` | The Radeon worker is busy, reinitialising or could not restore. Retry after `retry_after_ms` |
| 500 | `internal` | A host bug, also in the host log |

`not_streaming` appears only as the `reason` of a disallowed action in `GET /session`. Disconnect with no session at all is `409 no_session`. On a session that is already suspended it answers `200` again.

### Radeon settings

`GET /radeon` returns a `revision`, a panel `state`, `host_control` and `writable_now` for the caller, and `gpus` (today the one the host encodes on). Each setting has an `id` (`<group>.<field>`, for example `anti_lag.enabled` or `chill.max_fps`), a `kind` (`toggle`, `integer` with `range`, `choice` with `options`) and one of five states:

| State | Meaning |
|---|---|
| `configurable` | Readable and writable. The value is present |
| `read_only` | Readable, not writable on this driver |
| `needs_external_setup` | Turn it on once in AMD Software. `message` says how |
| `unsupported` | Not on this GPU or driver. No value |
| `temporarily_unavailable` | Try again later. `message` says why |

A setting is `configurable` only if the hardware probe proved it on the installed driver ([hardware matrix](hardware-matrix.md)). Clients ignore ids and fields they do not know.

A write is validated as a whole first (ids, types, range and step), so a bad request changes nothing. Before a setting changes for the first time in a session, the host writes its original value to a journal on disk. The changes then apply in order. If the driver refuses one, the earlier ones are undone and the answer is `driver_rejected`. The response is the re-read state, so settings the driver changed in turn show up too. While a change is pending, the setting carries `restore: { "original", "written" }`.

**Restore.** The host puts the original values back when the session ends (End session, Close game, grace period over, a stock `/cancel`, the stream ending with no grace), when the device is unpaired or disabled, on host shutdown, on `POST /radeon/restore`, and at the next host start if a journal survived a crash. It does not restore at Disconnect: a suspended session keeps its settings until the session ends, and resuming does not restore either. A setting is put back only if it still has the value the host wrote. A change made on the PC meanwhile wins and the entry is dropped. A restore that fails goes into the host log, stays in the journal, is retried, and keeps showing in `GET /radeon`.

### Versioning

The path carries the major version. Inside v1, changes only add: fields, features, setting ids, actions, error codes and enum values. Removing or renaming anything needs `/v2`, announced by a higher `NativeApiVersion`, and the host keeps serving v1 for the whole 3.x line. Clients ignore unknown fields, treat an unknown setting state as `unsupported` and an unknown session state as `idle` with no actions. The Rust types are the `rubylight-native-api` crate (`rust/core/native-api`), with JSON fixtures under `tests/fixtures`.

### Compatibility

Stock Moonlight and Nightjar 0.7.x never call these paths. Against a 3.0 host their `/serverinfo`, `/applist` and `/cancel` requests give the same answers as before apart from the new field, `/cancel` still needs only Launch permission, and reading the native API, or an action that is refused, changes nothing they can see. A successful End session or Close game does change `/serverinfo` (`currentgame`, `state`), as a stock quit would. The tests are in [compat.rs](../rust/host/src/nvhttp/native_tests/compat.rs) and [native_tests.rs](../rust/host/src/nvhttp/native_tests.rs). A Nightjar 3.0 client that finds the field missing shows the classic grid.
