# HTTP API

[Documentation](README.md) · [Getting started](getting-started.md) · [Configuration](configuration.md)

The host serves its administration API at `https://localhost:47990/api`; the web console is built on the same API. Changing the base `port` setting moves the console to `port + 1`.

Routing, authentication and token scopes are implemented in [web.rs](../rust/host/src/web.rs), and the handlers for each area in [web/](../rust/host/src/web). The [C++ host's API guide](https://github.com/RamazanKara/Rubylight/blob/2.0.0-rc.23/docs/legacy/api-cpp.md) is kept at the 2.0.0-rc.23 tag as a migration reference.

Paired clients use other routes, on the streaming HTTPS port (`HttpsPort` from `/serverinfo`, normally 47984), authenticated by the certificate the device paired with. None of the authentication methods on this page apply to them:

| Route | Reference |
| --- | --- |
| `/api/native/v1/...` | [Native API v1](native-api.md): game library, session state and actions, graphics driver settings (Radeon or NVIDIA), files, power and more. [Summary below](#native-api-v1-paired-clients). |
| `/api/hardware-metrics` | [Hardware metrics](hardware-metrics.md): CPU and AMD GPU readings for a client's overlay. |
| `/api/network-path` | [Remote connection route](network-path.md): the requesting peer's current Tailscale route category (`NetworkPathVersion`). |

Moonlight pairing, app discovery and streaming use the GameStream endpoints in [nvhttp.rs](../rust/host/src/nvhttp.rs).

## Authentication

Create the administrator account through the local console first. Initial account creation (`POST /api/password` while no account exists) is accepted only from the host PC. Access to the web interface also follows `origin_web_ui_allowed`: `pc`, `lan` (the default), or `wan`.

| Method | Use |
| --- | --- |
| HTTP Basic | Send the administrator username and password over HTTPS. Useful for interactive scripts. |
| Scoped API token | Send `Authorization: Bearer <token>`. Each token grants only the selected paths and HTTP methods. |
| Web session | Sign in with `POST /api/auth/login`. The host issues secure, HttpOnly session and refresh cookies; it also returns the access, refresh and CSRF tokens in JSON. |

The public API routes are `/api/auth/status`, `/api/auth/login`, `/api/auth/refresh`, `/api/csrf-token` and `/api/configLocale`. Other API routes require authentication, apart from local first-time account creation.

### Scoped tokens

Use the console's API token controls, or these administrator-authenticated routes. **Select all permissions** selects every available path and method, including paths hidden by search. You can clear the selection or deselect individual permissions before creating the token. A token may hold any number of the advertised paths, and the host keeps at most 256 tokens.

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

Choose paths exactly as returned by `/api/token/routes`, including the pattern entries (`/api/apps/[^/]+`, `/api/apps/[^/]+/cover`, `/api/apps/[^/]+/icon`, `/api/covers/[0-9]+`). The host validates the selected methods and matches each scope against the whole request path. Secrets are stored as hashes; keep the creation response if the token will be used later.

Some routes are not in the catalogue, so no token can reach them: the authentication and token routes themselves, and the [console-only routes](#console-only-routes).

### Sessions and CSRF

Browser sign-in takes `username`, `password` and optional `remember_me`. `POST /api/auth/refresh` rotates the refresh token; `POST /api/auth/logout` revokes the current session. `GET /api/auth/sessions` lists sessions, and `DELETE /api/auth/sessions/{id}` revokes one. Remembered sessions survive host restarts. `GET /api/auth/status` answers `authenticated`, `credentials_configured` and `login_required`, and renews a remembered browser whose access cookie lapsed.

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

`/api/metadata` (also served as `/api/meta`) reports the host version, codec probe status, configured capture backend, displays, GPUs, audio endpoints, virtual-display capabilities and host warnings. Results are cached for up to five seconds; reading them does not start a codec probe. Codec state (`encoder_status.state`) is `checking`, `ready` or `failed`.

`/api/session/status` includes `activeSessions`, `appRunning`, `appName`, `paused` and `running`. An app can remain running after its stream disconnects, so these fields describe different states.

## Endpoint groups

JSON requests use `Content-Type: application/json`; most responses include a Boolean `status`. Display lists, images and log downloads have their own response formats. Every route below can be granted to an API token unless it is listed under [Console-only routes](#console-only-routes).

| Area | Read | Change |
| --- | --- | --- |
| Host settings | `GET /api/config`, `GET /api/metadata` (or `/api/meta`), `GET /api/configLocale` | `POST` or `PATCH /api/config`, `POST /api/warnings/dismiss` |
| Applications | `GET /api/apps`, `GET /api/apps/{id}/cover`, `GET /api/apps/{uuid}/icon` | `POST /api/apps`, `DELETE /api/apps/{id}`, `POST /api/apps/delete`, `POST /api/apps/reorder`, `POST /api/apps/launch`, `POST /api/apps/close` |
| Game library | `GET /api/library/discovery` | `POST /api/library/import`, `POST /api/library/sync`, `POST /api/library/restore` |
| Paired devices | `GET /api/clients/list`, `GET /api/clients/pending` | `POST /api/pin`, `POST /api/otp`, `POST /api/pairing/qr` (with `qr_pairing` on, the default), `POST /api/clients/update`, `POST /api/clients/disconnect`, `POST /api/clients/unpair`, `POST /api/clients/unpair-all`, `POST /api/clients/reset-network-history` |
| Webhooks ([format](webhooks.md)) | `GET /api/webhooks` | `POST /api/webhooks`, `POST /api/webhooks/delete`, `POST /api/webhooks/test` |
| Active sessions | `GET /api/session/status`, `GET /api/rtsp/sessions`, `GET /api/session/console` | `POST /api/session/console` |
| Forwarded USB devices | `GET /api/usb` | `POST /api/usb/detach` |
| Displays | `GET /api/display-devices`, `GET /api/clients/display-layout`, `GET /api/clients/hdr-profiles`, `GET /api/display/golden_status` | `PUT /api/clients/display-layout`, `POST /api/display/export_golden`, `POST /api/display/restore_golden`, `DELETE /api/display/golden`, `POST /api/display/terminate_virtual`, `POST /api/reset-display-device-persistence` |
| Frame limiting and HDR | `GET /api/frame-limiter/status`, `GET /api/rtss/status`, `GET /api/health/vulkan-hdr-layer`, `GET /api/framegen/edid-refresh?device_id=...` | `POST /api/health/vulkan-hdr-layer/register` |
| Steam | `GET /api/steam/status`, `GET /api/steam/games` | `POST /api/steam/force_sync`, `POST /api/steam/launch` |
| Playnite | `GET /api/playnite/status`, `GET /api/playnite/games`, `GET /api/playnite/categories` | `POST /api/playnite/install`, `POST /api/playnite/uninstall`, `POST /api/playnite/force_sync`, `POST /api/playnite/cover`, `POST /api/playnite/launch` |
| Library extras | `GET /api/covers/{id}`, `GET /api/lossless_scaling/status`, `GET /api/browse?path=...&type=...` | `POST /api/covers/upload`, `POST /api/apps/purge_autosync` |
| Graphics driver (Radeon or NVIDIA) | `GET /api/radeon` | Set `radeon-profile` and `radeon-settings` on the application. |
| Updates | `GET /api/updates` | `POST /api/updates/check`, `POST /api/updates/install`, `POST /api/updates/install_now`, `POST /api/updates/cancel` |
| Diagnostics | `GET /api/logs`, `GET /api/logs/tail`, `GET /api/logs/analysis`, `GET /api/logs/export`, `GET /api/logs/export_crash`, `GET /api/logs/export_crash/manifest`, `GET /api/health/crashdump` | `POST /api/health/crashdump/dismiss` |
| Administration | Authentication and token routes above. | `POST /api/password`, `POST /api/restart`, `POST /api/quit` |

Request and response details:

- **Configuration.** Writes merge the submitted fields and return `restart_required` (`true` unless every submitted key applies without a restart) and an optional `warning`. Treat the response as a saved configuration, not confirmation that every active stream has adopted it. The `webhooks` setting is left out of reads and refused in writes; use the webhook routes. `POST /api/warnings/dismiss` takes the `code` of a host warning that `/api/metadata` lists.
- **Applications.** Applications are updated by their `uuid`. `DELETE /api/apps/{id}` takes the UUID or numeric app ID; `POST /api/apps/delete` takes one `uuid`, or a `uuids` array (at most 10,000) to delete several in one save, and answers `deleted` and `kept`. `POST /api/apps/reorder` takes an `order` array. Read `/api/apps` before making changes rather than treating an array index as the identity. `/api/apps/{uuid}/icon` returns the PNG icon a Playnite sync saved for the app, or `404`.
- **Game library.** `GET /api/library/discovery` lists the games the launcher scan found. `POST /api/library/import` takes `games`, an array of `{"source", "id"}` pairs; `POST /api/library/sync` rescans; `POST /api/library/restore` brings back removed games, `{"all": true}` or `{"games": ["steam:570", ...]}` (at most 10,000).
- **Device updates.** `POST /api/clients/update` takes the device's `uuid` and the fields to change. `name`, `enabled` and `perm` (masked with `0x071f1f00`) change the device; any other key is stored in the device's settings, and `null` removes it. The grants `host_control` ([host control](configuration.md#host-control-for-a-device)), `pairing_approve` (**Approve new devices**, see [approve a device](native-api.md#approve-a-device-pairingapprove)), `allow_sleep` (Sleep PC), `allow_restart_shutdown` (Restart and shut down PC), `allow_usb` (USB forwarding) and `coop_join` (Can join co-op games) accept only `true`, `false` or `null`, and only a JSON `true` turns one on; anything else refuses the whole update. `shared_folders` takes `{"<folder id>": "read" | "write"}` or `null` ([shared folders](native-api.md#shared-folders)), and `config_overrides` is checked as the host's own settings are. The first device with a certificate paired on a host starts with `pairing_approve` unless the owner chose **Watch only**; every other device gets it only here. `connected`, `network_loss_routes`, `client_agent` and `client_warning` are read-only.
- **Device list.** `/api/clients/list` has, for each device, read-only `network_loss_routes` (address, seconds until expiry, and eligible sessions until a fast-rate probe), `client_agent` (the `User-Agent` of the device's latest `/serverinfo` since the host started, or `null`) and `client_warning` (`{"problem", "link"}` for a client build with a known problem against this host, or `null`; it never refuses the device). The top-level `sleep_unavailable` says why this PC offers no Sleep PC, such as a PC with only Modern Standby, or is `null`. `POST /api/clients/reset-network-history` with `{"uuid": "device-id"}` clears the loss routes for subsequent streams; it does not change a running stream.
- **USB.** `GET /api/usb` lists the `devices` that clients forward: `id`, `client`, `name`, `vid`, `pid`, `class`, `speed`, `busid`, `state` (`waiting` for the tunnel, `opening`, `attached` or `detaching`), the driver's `port` once attached, and `since_unix_ms`. `POST /api/usb/detach` with `{"id": 1}` detaches one; the client is told.
- **Console session recovery.** `GET /api/session/console` returns `recovery` with `session_id`, `console_session_id`, `can_reattach` and a human-readable `reason`. After explicit user confirmation, `POST /api/session/console` with those two IDs asks Windows to return the host's own RDP session to the physical console. The destination must have no signed-in user, or be disconnected with a verified matching user SID; active sessions, different users and unknown identities are refused. The host rechecks session identity, destination eligibility and idle transports before making the request, and never selects another user's source session. The response acknowledges the request, not completion of the asynchronous Windows transition. This can disconnect RDP and make the desktop visible at the PC.
- **Graphics driver.** `GET /api/radeon` lists the graphics driver settings of the GPU the host streams with, Radeon or NVIDIA (`nvidia_` ids), as the [native API](native-api.md#radeon-state-and-writes) does (id, state, range, options), for a game's **Graphics driver** panel. It only reads; a game's profile is applied when it starts.
- **File picker.** `/api/browse` lists a folder of this PC: `path`, `parent` and `entries` (each with `name`, `path` and `type` `directory` or `file`), folders first. `type=executable` lists only `.exe`, `.bat`, `.cmd` and `.ps1` files; no `type`, `type=any` or `type=file` any file; `type=directory` or any other value no files. Folders are always listed. A file or missing path lists the nearest existing folder above it; an empty `path` lists the drives, and a drive root's `parent` is empty.
- **Playnite.** `/api/playnite/cover` takes `playnite_id` and `cover_key` (an image saved by `/api/covers/upload` or a cover search), asks the Playnite plugin to use that image as the game's cover, then syncs the library and returns the cover's `path`. `/api/playnite/launch` restarts Playnite in desktop mode.
- **Updates.** `/api/updates/install` queues the update for an idle installed service. `/api/updates/install_now` installs without waiting and ends a running stream; `now` is `true` in `/api/updates` while such an update is queued. Read `/api/updates` for progress and the result.
- **Logs.** `/api/logs` and `/api/logs/export` return up to the last 8 MiB of the current log as text; the ordinary log export is not a ZIP. `/api/logs/export_crash` creates the support ZIP. `/api/logs/tail` accepts `offset` and `max`, then returns `text`, the next `offset`, `size` and a `reset` flag for log rotation.
- **Log analysis.** `/api/logs/analysis` reads the last 8 MiB of the current log on the host and returns `findings`, newest first, plus the `since` and `until` times it covered. Each finding has `id`, `category` (`encoder`, `capture`, `network` or `driver`), `level` (`warn` or `error`), `title`, `explanation`, `fix`, a `link` to the troubleshooting guide, `count`, `first_seen`, `last_seen` and the latest matching line as `sample`. It uses fixed local rules and contacts nothing.

## Console-only routes

These routes back console pages. They take Basic or web-session authentication like the others, but they are not in the token catalogue, so no API token reaches them.

| Area | Read | Change |
| --- | --- | --- |
| Stream guests ([guests](browser-guests.md#for-developers)) | `GET /api/guests`, `GET /api/guests/addresses` | `POST /api/guests/invite`, `POST /api/guests/revoke`, `POST /api/guests/kick` |
| Co-op ([Nucleus Co-op](nucleus-coop.md)) | `GET /api/coop` | `POST /api/coop/folder`, `POST /api/coop/open`, `POST /api/coop/close`, `POST /api/coop/kick`, `POST /api/coop/end` |
| Neural Rendering ([setting](configuration.md#neural-rendering)) | `GET /api/neural?uuid=...` | `POST /api/neural/accept`, `POST /api/neural/enable`, `POST /api/neural/disable` |
| Managed game files ([files](configuration.md#managed-game-files)) | `GET /api/game-settings?uuid=...` | `POST /api/game-settings/release` |
| Wake and sleep ([power](configuration.md#sleep-pc-and-sleep-after-idle)) | `GET /api/power` | `POST /api/power/wake/fix` |
| Removed child sessions | `GET /api/child-session` | `POST /api/child-session/restore` |
| Stream in the console ([WebRTC](webrtc.md#stream-in-the-console)) | `GET /api/webrtc/signal` (WebSocket, web session only) | |

- **Co-op.** `GET /api/coop` shows the background mode and why, the Nucleus Co-op install (the chosen folder, or why it can't run from it), each app's co-op status and every co-op session. `POST /api/coop/folder` checks the folder in `path` and keeps it as `nucleus_path`. `open` and `close` open and close Nucleus Co-op's window on the PC. `kick` takes `session` and `slot` and removes that player; `end` takes `session` and ends it; both end the streams those seats held.
- **Neural Rendering.** `GET /api/neural?uuid=` says whether this PC can have it and why not, whether the notice was accepted, and the app's install with its progress. `POST /api/neural/accept` records acceptance of the notice. `enable` (`uuid`, optional `exe`, the full path of a `.exe`) downloads and installs; `disable` (`uuid`) removes. Both answer `202`, and `409` before the notice is accepted, while a stream runs, while the game is busy, or when the PC can't have it.
- **Managed game files.** `GET /api/game-settings?uuid=` lists which devices have a saved copy of the app's files. `POST /api/game-settings/release` with `{"id": "..."}` puts the files back now, for a swap that waits for a process that is not the game; `409` with the reason when it can't.
- **Wake and sleep.** `GET /api/power` reads Wake-on-LAN readiness and sleep after idle; `?refresh=1` reads the adapters again. `POST /api/power/wake/fix` changes adapter and power settings only when asked: optional `fast_startup_off` (Boolean) and `changes` (the changes `GET /api/power` listed, to apply exactly those). It answers `202` with every change it will make, applied about a second later; `200` when there is nothing to change; `409` while a stream is active or starting, or a fix runs.
- **Removed child sessions.** Background streaming in a Windows child session was removed. `GET /api/child-session` says whether Windows settings its setup changed are still changed (`pending`) and why the last restore failed (`problem`); `POST /api/child-session/restore` restores them again and answers the same.

## Errors

Authentication failures return HTTP `401`; denied origins or network reach return `403`; invalid requests commonly return `400` with `{"status":false,"error":"..."}`. Login rate limiting returns `429` for the sign-in endpoint and `401` for HTTP Basic authentication. Check the HTTP status and response body together.

For exact request fields beyond this overview, follow [the API router](../rust/host/src/web.rs) and [its area handlers](../rust/host/src/web), [token scope validation](../rust/core/src/auth.rs) and [administration checks](../rust/tests/web_api.py).

## Native API v1 (paired clients)

Rubylight 3.0 and later. Nightjar and other paired clients use `/api/native/v1` on the streaming HTTPS port. No Basic authentication, cookies or API tokens apply, and nothing is served on plain HTTP or the console port: the client presents the TLS certificate it paired with, and the host checks the device's permissions on every request. `GET /serverinfo` carries `NativeApiVersion`: missing on hosts without the API, `0` for a request that may not use it, and `1` when a paired HTTPS request may call `/api/native/v1/capabilities`. Stock Moonlight ignores the field.

[Native API v1](native-api.md) is the contract: discovery, features, every endpoint, permissions, errors and versioning. **Host control**, **Approve new devices** and the other per-device grants it needs are set only in the console (**Devices** → device → **Other**) or through `POST /api/clients/update` above.
