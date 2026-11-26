# Item 08: Quick Suspend & Resume (`quick-suspend`)

Status: design, not implemented (Wave 2, Rubylight & Nightjar 7.0.0). Rebased 2026-11-25 onto post-refactoring `origin/main` `955ef39598` (`v6.4.1`) and [protocol reservations](00-protocol-reservations.md).

> **As built (explicit suspend and resume, 7.0):** the shipped wire and API are in [control-messages.md](../control-messages.md#0x554b-quick-suspend-9-or-20-bytes) and [native-api.md](../native-api.md), and they win over sections 2.2 and 2.3 below. Request `flags` are 0 (no `disconnect_after_suspend`); `POST /session/suspend` and `/session/resume` take `{}` and answer 200 with `QuickSuspendStatus` (`state`, `reason`, `message`, `app_title`, `suspended_count`, `suspended_since_unix_ms`, `suspended_by`), a refusal included; the setting is `off` or `manual`, host-wide. The journal is its own `quick-suspend.json` with a watchdog helper (`--quick-suspend-watch`) instead of the shared recovery journal and a service hook. Not built: `on_disconnect`, per-app overrides, audio muting, `game_frozen` in the session snapshot and the tray banner.

## 1. Summary & User Experience

Handheld consoles let players put a single-player game to sleep in one tap and pick up right where they left off minutes or hours later — without burning GPU power, spinning fans, or risking unpaused gameplay while disconnected. **Quick Suspend & Resume** brings that workflow cleanly to Rubylight & Nightjar in pure Rust (zero third-party suspend binaries or external mods):

- **Explicit or automatic freeze:** For opted-in single-player games, a player can tap **Suspend Game** in Nightjar's Stream Menu (sending `0x554B` or `POST /api/native/v1/session/suspend`) or let Rubylight automatically freeze the game when the client disconnects (`quick_suspend = on_disconnect`).
- **What happens during Quick Suspend:**
  1. Rubylight validates the running app's verified owned process tree against a **strict, non-overridable safety denylist** (anti-cheat engines, protected processes, launchers, overlays, system processes, active guests/co-op).
  2. **Write-ahead durability first:** Before freezing a single thread, Rubylight atomically writes and `sync_all()`s the exact `(pid, created_100ns, exe_name, prev_audio_muted)` records into the canonical `RecoveryJournalV2` (`rust/core/session/src/recovery_journal.rs`, persisted atomically to `recovery-journal.json` alongside `session.json`, carrying `suspended_pids` plus display/DPI/audio state — unified with `17-ux-craft-and-resilience.md`) and mirrors `game_frozen` into `session.json`.
  3. Rubylight cleanly mutes the game's WASAPI audio session (preventing stuck audio-buffer buzz), freezes the game's verified process tree via `ntdll.dll`'s `NtSuspendProcess`, and drops the stream's capture & encoder while keeping `SessionState::Suspended` (and keeping the display topology stable so the frozen game's D3D11/D3D12 device is never lost!). GPU 3D/VCN utilization drops to near 0% as the frozen game stops submitting frames.
- **Instant Resume:** When the player reconnects (`/resume`) or taps **Resume** (`0x554B` / `POST /api/native/v1/session/resume`), Rubylight verifies each recorded `(pid, creation_time_100ns)` still matches the exact same process, calls `NtResumeProcess`, restores the original WASAPI mute state, removes the journal entry, and resumes streaming seamlessly.
- **Safety-first guarantees:**
  - **Opt-in / default-off per app (`off` by default):** Rubylight NEVER freezes a game unless the user opted it in.
  - **Anti-cheat & multiplayer protection:** Any anti-cheat service/process, protected process (`PPL`), launcher, system process, or active guest/co-op session is loudly refused — never partially frozen.
  - **Crash-proof recovery:** If the host crashes, panics, is force-killed, or restarts unexpectedly while a game is frozen, both host startup recovery AND the Windows service watchdog (`butterpollo-service.exe`) automatically unfreeze every process Rubylight suspended so no game is ever left permanently frozen.

---

## 2. Protocol, Native API & Capability Contract

All identifiers strictly follow [00-protocol-reservations.md](00-protocol-reservations.md). `PROTOCOL_VERSION` and `rubylight_native_api::API_VERSION` (`1`) remain **unchanged**.

### 2.1 Discovery & Capability Gating

| Surface | Field / Constant | Value |
|---|---|---|
| `/serverinfo` | `QuickSuspendVersion` | `1` (emitted in `Offered::serverinfo_fields`) |
| RTSP `DESCRIBE` | `a=x-rl-control:0x554B` | Emitted in `Offered::describe_lines` |
| Native API `/capabilities` | `feature::QUICK_SUSPEND` = `"session.quick_suspend"` | Listed for every paired caller on a 7.0.0+ host |
| Nightjar Rust | `HostCapabilities::Feature::QuickSuspend` | Proven by `QuickSuspendVersion >= 1` **and** native `session.quick_suspend` |
| Nightjar Java | `HostCapabilities.Feature.QUICK_SUSPEND` | Proven by `QuickSuspendVersion >= 1` **and** native `session.quick_suspend` |

### 2.2 Control-Stream Message `0x554B` (`QUICK_SUSPEND_MESSAGE_TYPE`)

- **ID:** `0x554B` in `RUBYLIGHT_7_CONTROL_IDS` (`0x5548..=0x554F`), bit 15 clear.
- **Direction:** Bidirectional.
- **Delivery:** Reliable encrypted ENet control channel (unauthenticated plaintext control packets are dropped).
- **Module:** `client/protocol/core/src/quick_suspend.rs` (`no_std`).

#### Wire Format (little-endian)

| Offset | Size | Type | Field | Semantics |
|---|---|---|---|---|
| 0 | 1 | `u8` | `version` | `1` |
| 1 | 1 | `u8` | `kind` | `1` = `ClientRequest`, `2` = `HostState` |
| 2 | 2 | `u16` | `flags` | Bit 0 (`0x0001`): `disconnect_after_suspend` (default `1` for Stream Menu "Suspend & Disconnect") |
| 4 | 4 | `u32` | `request_id` | Client monotonic nonce (`0` for host-initiated state push) |
| 8 | var | bytes | `body` | Tagged by `kind` below |

- **`kind = 1` (`ClientRequest`, Client -> Host):**
  - `body[0]`: `op` (`1 = QueryState`, `2 = Suspend`, `3 = Resume`).
  - Only the session owner (or a device with `host_control`) may suspend/resume; guests and `InputOnly` streams receive `SuspendReason::NotOwner`.
- **`kind = 2` (`HostState`, Host -> Client):**
  - `body[0]`: `state` (`0 = Unavailable`, `1 = Ready`, `2 = Suspended`, `3 = Resuming`, `4 = Refused`).
  - `body[1]`: `reason` (`SuspendReason` `u8`):
    - `0 = None`
    - `1 = DisabledForApp` (app's `quick_suspend` setting is `off`)
    - `2 = NoOwnedProcess` (Desktop app or detached command with no tracked game PIDs)
    - `3 = AntiCheatDetected` (anti-cheat process/service/driver detected)
    - `4 = ProtectedProcess` (`ACCESS_DENIED` / PPL process)
    - `5 = DenylistedProcess` (launcher/system/critical process)
    - `6 = GuestsActive` (multi-client guests are watching/playing)
    - `7 = CoopSession` (multi-instance co-op session active)
    - `8 = NotOwner` / `PermissionDenied`
    - `9 = SessionBusy` (launch or teardown in progress)
    - `10 = NoSession`
    - `11 = IoOrOsError`
  - `body[2..4]` (`u16` LE): `suspended_pid_count` (`0` unless `state == Suspended`).
  - `body[4..12]` (`u64` LE): `suspended_since_unix_ms` (`0` unless `state == Suspended`).

### 2.3 Native API v1 Routes (`client/protocol/native-api/src/quick_suspend.rs`)

Mounted under `/api/native/v1`, authenticated by paired TLS certificate, checking live device permissions on every call:

| Method & Route | Permission | Request / Response |
|---|---|---|
| `GET /api/native/v1/session/suspend` | `view` | Returns `QuickSuspendStatus { state, eligible, reason, message, app_policy, suspended_pid_count, suspended_since_unix_ms, owner }` |
| `POST /api/native/v1/session/suspend` | `launch` (session owner) or `host_control` | Request `{"disconnect": true}` (`#[serde(deny_unknown_fields)]`, default `disconnect = true`). Freezes verified game process tree, optionally disconnects stream into `State::Suspended`, returns `QuickSuspendStatus` (or `409 Conflict` with stable `reason` & plain-English `message` if refused). |
| `POST /api/native/v1/session/resume` | `launch` (session owner) or `host_control` | Request `{}` (`#[serde(deny_unknown_fields)]`). Idempotently resumes all suspended game processes and restores game audio mute; returns `QuickSuspendStatus`. (Note: standard `/resume` launch also auto-resumes frozen processes before the stream attaches!) |

Also extends `GET /api/native/v1/session` (`SessionSnapshot`) additively with `#[serde(default)] pub game_frozen: bool` so Nightjar's Console Home / PC card immediately displays a **"Suspended — Resume"** badge without an extra round-trip!

---

## 3. Architecture & <=800-LOC Module Plan

### 3.1 Windows Process Suspension & Safety Architecture

#### Narrow Win32/NT wrapper (`rust/windows/io/src/process/suspend.rs`)

- **Dynamic System32 loading:** Loads `ntdll.dll` via `LOAD_LIBRARY_SEARCH_SYSTEM32` (matching `nvenc.rs::system_library`) and resolves:
  - `NtSuspendProcess(HANDLE) -> NTSTATUS`
  - `NtResumeProcess(HANDLE) -> NTSTATUS`
- **Strict PID + creation-time identity verification:** Before EVERY `NtSuspendProcess` AND EVERY `NtResumeProcess`:
  1. Open process with `OpenProcess(PROCESS_SUSPEND_RESUME | PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid)`.
  2. Call `GetProcessTimes` and compare the 64-bit `CreationTime` (`FILETIME`, 100 ns ticks) against the expected `started: u64` from `RunningApp::owned_processes` (or the persisted journal).
  3. If `CreationTime != expected_started`, the original process exited and Windows reused the PID -> **NEVER suspend or resume it**; skip cleanly and log info.
- **Atomic all-or-nothing tree freeze with rollback:**
  1. Resolve target process list from `RunningApp::owned_processes` (`rust/host/stream/src/process.rs:469`: Steam tracker, Playnite tracker, Windows Job object `child.process_ids()`, verified `foreign` PIDs). If empty (e.g. Desktop or untracked detached app), refuse with `NoOwnedProcess`.
  2. Filter out known launcher parent executables (never freeze `steam.exe`, `steamwebhelper.exe`, `EpicGamesLauncher.exe`, `Playnite*.exe`, `GalaxyClient.exe`, `EADesktop.exe`, `upc.exe`, `Battle.net.exe`). If zero game processes remain after filtering launchers, refuse with `NoOwnedProcess`.
  3. Run the **hard safety check** (`butterpollo_core::quick_suspend::evaluate_safety`) over BOTH the target tree AND active host processes/services:
     - **Anti-cheat hard denylist (process names & installed/running service names):** `EasyAntiCheat*`, `EACLauncher*`, `BEService`, `BEDaisy`, `vgc`, `vgtray`, `vgk`, `EAAntiCheat*`, `FACEITService`, `PnkBstrA`, `PnkBstrB`, `GameMon*`, `xhunter1`, `mhyprot*`, `HoYoKProtect`, `randgrid`, `atvi-brynhildr`, `denuvo-anti-cheat`. Also checks if any target process has `ProcessProtectionInformation` (PPL/Protected) -> refuses with `AntiCheatDetected` / `ProtectedProcess`.
     - **System / critical process denylist:** PID `<= 4`, Session 0 processes, `csrss.exe`, `winlogon.exe`, `dwm.exe`, `explorer.exe`, `lsass.exe`, `smss.exe`, `services.exe`, `svchost.exe`, `butterpollo.exe`, `butterpollo-service.exe`, `conhost.exe`, `audiodg.exe`.
     - **Session state guards:** Active multi-client guests (`GuestsActive`), active co-op session (`CoopSession`), or launch/teardown in progress (`SessionBusy`).
  4. **Write-ahead journal BEFORE freeze:** Synchronously update `RecoveryJournalV2` (`rust/core/session/src/recovery_journal.rs`) and write + `File::sync_all()` + atomic rename `recovery-journal.json` (alongside `session.json`) containing every `suspended_pids` record `(pid, created_100ns, app_id, app_title, exe, suspend_count, prev_audio_muted, suspended_unix_ms)` plus any active display/DPI/audio recovery state.
  5. **Mute game audio cleanly:** Using WASAPI `IAudioSessionManager2` (`rust/windows/io/src/audio_app_route.rs` / new `audio_mute.rs`), locate audio sessions belonging to target PIDs, record each session's current `GetMute()`, and call `SetMute(TRUE)`. (Prevents frozen games whose DirectSound/WASAPI ring buffer wasn't flushed from looping a 20 ms buzz on host speakers!)
  6. **Suspend in leaf-first order** (children before parents, preventing parent crash-reporters/watchdogs from thinking a child hung): if ANY `NtSuspendProcess` returns non-zero `NTSTATUS` (e.g. `STATUS_ACCESS_DENIED`), **immediately roll back** by calling `NtResumeProcess` on every already-suspended PID, restoring audio mute, clearing `suspended_pids` in `recovery-journal.json` (deleting the file if no display/DPI/audio entries remain), and returning `ProtectedProcess` / `IoOrOsError`.
- **Why display topology stays intact while suspended (critical GPU insight):**
  - When a client disconnects after Quick Suspend, the stream stops capture and destroys the encoder (`video/teardown.rs`), dropping host capture/encode GPU load to `0%`.
  - Meanwhile, the host keeps `State::Suspended` with the game's retained display (`app_display`, with an extended suspend grace timeout or `dd_hold_layout_while_paused = true` semantics while frozen). **We deliberately do NOT unplug or reconfigure the virtual/physical display while the game is frozen**, because changing DXGI display topology while a D3D11/D3D12 game's threads are suspended risks a swap-chain / `DXGI_ERROR_DEVICE_REMOVED` crash on resume! Only optional monitor DPMS blanking (for physical displays, never detaching the adapter output) is safe and opt-in.

### 3.2 `<=800-LOC` Module Plan & `size_ratchet.json` Compliance

**Critical `size_ratchet.json` constraint spotted:** `rust/host/stream/src/session_control.rs` is currently **774 LOC** and is **NOT grandfathered** in `tools/size_ratchet.json` (hard ceiling = `800` LOC, only 26 lines of headroom!). Therefore, ALL Quick Suspend orchestration on `Host` MUST live in a brand-new sibling module `rust/host/stream/src/session_control/quick_suspend.rs`, with at most `<= 10` lines of delegation inside `session_control.rs`!

#### Grandfathered files touched (from `tools/size_ratchet.json`)

| Grandfathered file | Ratchet ceiling | How we avoid growing it |
|---|---|---|
| `rust/host/stream/src/process.rs` | `2064` | Expose existing `RunningApp::owned_processes` as `pub(crate)` (<=4 lines; zero new logic in `process.rs`) |
| `rust/windows/io/src/process.rs` | `1725` | Add `pub mod suspend;` (1 line! All `ntdll` suspend/resume/safety/PPL logic lives in `rust/windows/io/src/process/suspend.rs`) |
| `rust/host/stream/src/stream/control.rs` | `2675` | Single match arm `QUICK_SUSPEND_MESSAGE_TYPE => crate::session_control::quick_suspend::handle_control(...)` (<=6 lines) |
| `rust/host/web/src/native_api.rs` | `853` | Add `mod quick_suspend;`, 3 routes in `router()`, and `feature::QUICK_SUSPEND` (<=8 lines) |
| `rust/host/src/main.rs` | `914` | Add `--recover-session` CLI flag dispatch & startup recovery call over `RecoveryJournalV2` (<=8 lines) |
| `rust/host/stream/src/state.rs` | `2247` | Add `pub quick_suspend: Mutex<crate::session_control::quick_suspend::Tracker>` (<=3 lines) |
| `rust/service/src/supervisor.rs` | `827` | Invoke canonical `--recover-session` on unexpected host exit (<=8 lines, plus `rust/service/Cargo.toml` version bump + `python rust/release/service_inputs.py --update` per `rust/release/README.md`) |
| `rust/core/config/src/config.rs` | `1373` | Add 4-line `pub fn quick_suspend(&self)` delegating to `crate::quick_suspend::Policy::parse` |
| `client/protocol/policy/src/http/capabilities.rs` | `1257` | Add `QuickSuspend` variant (<=4 lines) |
| `client/protocol/client/src/http/native.rs` | `821` | Put Quick Suspend HTTP calls in new `client/protocol/client/src/http/native_suspend.rs` (1 `mod` line) |
| `client/app/src/main/java/com/limelight/nvstream/http/HostCapabilities.java` | `1347` | Add `QUICK_SUSPEND` variant (<=4 lines) |
| `client/app/src/main/java/com/limelight/ui/StreamMenu.java` | `1321` | Delegate Suspend Game tile to new `StreamMenuSuspendSection.java` (<=6 lines) |
| `client/protocol/desktop-media/src/stream/mod.rs` | `2914` | Delegate `0x554B` events/commands to new `stream/quick_suspend.rs` (<=8 lines) |
| `client/protocol/desktop-config/src/strings/*.rs` | `1750` | **Zero growth!** Route all new 7.0.0 Quick Suspend Desktop translations to `client/protocol/desktop-config/src/strings_70/<code>.rs` (all 14 non-English locale tables; English literals live at call sites; never ad-hoc files or `strings/*.rs`) |

#### New `<=800-LOC` modules to create

| Planned Path | Est. LOC | Ownership (`//!` / `/**`) |
|---|---|---|
| `client/protocol/core/src/quick_suspend.rs` | ~200 | Wire codecs for `0x554B` (`QUICK_SUSPEND_MESSAGE_TYPE`), `ClientRequest`, `HostState`, `SuspendReason`, round-trip tests |
| `client/protocol/native-api/src/quick_suspend.rs` | ~160 | Native API v1 types (`QuickSuspendStatus`, `SuspendRequest`, `ResumeRequest`, `QuickSuspendState`, `SuspendReason`) + fixture tests |
| `rust/core/session/src/recovery_journal.rs` | ~220 | Canonical `RecoveryJournalV2` schema (`suspended_pids` plus display/DPI/audio state), atomic `recovery-journal.json` persistence helpers alongside `session.json`, shared by Item 08 and Item 17 (`17-ux-craft-and-resilience.md`) |
| `rust/core/session/src/quick_suspend.rs` | ~420 | Pure OS-free policy (`Policy { Off, Manual, OnDisconnect }`), non-overridable anti-cheat/launcher/system denylist, process-tree filter, leaf-first ordering, `SuspendedPidRecord` mapping into `RecoveryJournalV2`, state transitions, full unit tests |
| `rust/windows/io/src/process/suspend.rs` | ~380 | Narrow System32 `ntdll.dll` `NtSuspendProcess`/`NtResumeProcess` wrapper, `GetProcessTimes` PID-identity verifier, PPL/protected check, anti-cheat service probe, atomic rollback |
| `rust/windows/io/src/audio_mute.rs` | ~220 | Per-PID WASAPI `IAudioSessionManager2` mute/restore helper (best-effort with logged warning; never blocks suspend/resume) |
| `rust/host/stream/src/session_control/quick_suspend.rs` | ~460 | Host Quick Suspend manager: synchronous write-ahead `RecoveryJournalV2` (`recovery-journal.json` alongside `session.json`, `sync_all` + rename), suspend/resume orchestration, `Drop` failsafe, auto-suspend on disconnect, auto-resume on `/resume`, Close-game/End-session auto-resume before terminate, `0x554B` handler |
| `rust/host/web/src/native_api/quick_suspend.rs` | ~200 | Axum handlers for `GET/POST /session/suspend` and `POST /session/resume` |
| `client/protocol/client/src/http/native_suspend.rs` | ~140 | `nightjar-core` HTTP wrappers for `/api/native/v1/session/suspend` and `/resume` |
| `client/protocol/desktop-media/src/stream/quick_suspend.rs` | ~220 | Desktop Stream Menu Suspend Game tile, Home "Suspended — Resume" badge, `0x554B` dispatch |
| `client/app/src/main/java/com/limelight/ui/StreamMenuSuspendSection.java` | ~220 | Android Stream Menu Suspend Game tile + plain-English refusal dialog/toast |
| `client/protocol/desktop-config/src/strings_70/<code>.rs` (14 locale files) | ~35/locale | All 14 non-English 7.0.0 Desktop translation tables for Quick Suspend & Resume (`de`, `es`, `fr`, `it`, `ja`, `ko`, `pl`, `pt_br`, `ru`, `tr`, `uk`, `vi`, `zh_hans`, `zh_hant`; English literals live at call sites; never ad-hoc files or grandfathered `strings/*.rs`) |

*(Note: `rust/host/stream/src/session_control.rs` stays `<= 785` LOC, `rust/core/session/src/session_state.rs` stays `<= 430` LOC, and `rust/host/stream/src/service_tasks.rs` stays `<= 180` LOC — all well under 800 LOC!)*

---

## 4. State Machine, Reconnect Matrix & Crash-Restore Journal

### 4.1 Quick Suspend Lifecycle State Machine

```text
                    ┌──────────────────┐
                    │   Running (Live) │
                    └────────┬─────────┘
                             │ Explicit Suspend (0x554B / POST)
                             │ OR Client Disconnect (when app policy == on_disconnect)
                             ▼
              ┌──────────────────────────────┐
              │ 1. Evaluate Safety & Tree    │──(denylisted / PPL / guests / no PID)──► Refused (loud reason)
              └──────────────┬───────────────┘
                             │ eligible
                             ▼
              ┌──────────────────────────────┐
              │ 2. Write-Ahead Journal       │ (RecoveryJournalV2 -> recovery-journal.json
              │    + Mirror to session.json  │  + sync_all + atomic rename BEFORE NtSuspendProcess!)
              └──────────────┬───────────────┘
                             │ durable on disk
                             ▼
              ┌──────────────────────────────┐
              │ 3. Mute Audio + Leaf-First   │──(any NtSuspendProcess fails)──► Rollback all frozen PIDs,
              │    NtSuspendProcess          │                                  unmute, clear suspended_pids -> Refused
              └──────────────┬───────────────┘
                             │ all PIDs frozen
                             ▼
              ┌──────────────────────────────┐
              │      Frozen (Suspended)      │ (Stream disconnected -> capture/encoder stopped -> GPU idle;
              │   SessionState::Suspended    │  display retained; game_frozen = true)
              └──────────────┬───────────────┘
                             │ Any trigger:
                             │  - Client /resume or POST /session/resume or 0x554B Resume
                             │  - End session / Close game / Grace period expiry
                             │  - Host clean shutdown / panic Drop
                             │  - Host crash -> startup recovery OR service watchdog (--recover-session)
                             ▼
              ┌──────────────────────────────┐
              │ 4. Verify (pid, created_100ns)│
              │    + NtResumeProcess (all)   │
              │    + Restore Audio Mute      │
              │    + Clear suspended_pids    │
              └──────────────┬───────────────┘
                             ▼
                    ┌──────────────────┐
                    │   Running / Idle │
                    └──────────────────┘
```

### 4.2 Reconnect & Lifecycle Matrix

| Scenario | Host & Process Behavior |
|---|---|
| **Client taps "Suspend Game" in Stream Menu (`disconnect = true`)** | Safety check -> write-ahead journal -> mute audio -> freeze tree -> send `HostState::Suspended` over `0x554B` -> disconnect stream into `State::Suspended` (display retained, GPU capture/encode released). |
| **Client disconnects unexpectedly / network drops (`on_disconnect` enabled for app)** | After stream teardown enters `State::Suspended` (and after a short 3 s roam grace so a brief Wi-Fi roam doesn't flicker-freeze the game!), host automatically freezes the game tree. If the client reconnects within the 3 s window, no freeze occurs. |
| **Client reconnects (`/resume`) or Continue Here** | `launch_steps.rs` / `session_control` calls `quick_suspend.resume_all()` **before** opening capture/encoder, guaranteeing the game is already actively rendering when the first frame is captured. |
| **User triggers "End session" (keep game running on PC)** | Host **MUST resume all frozen game processes** and restore audio mute before releasing the display, so the user returning to their desk never finds a frozen game! |
| **User triggers "Close game" (`/cancel` or Native API)** | Host resumes frozen processes first (so graceful `WM_CLOSE` / save-on-exit handlers can execute!) before proceeding with normal close/terminate timeout. |
| **Suspend grace period expires (`dd_paused_virtual_display_timeout_secs`)** | Host resumes all frozen game processes and restores audio mute BEFORE tearing down the retained display (preventing a frozen game from crashing on display removal). |
| **User walks up to the PC physically while game is suspended** | Tray menu & Console 2.0 show a prominent **"Resume suspended game (<Title>)"** banner/action (one click calls `resume_all()`); also optional hotkey or automatic unfreeze if user presses `Alt+Tab` / focuses the suspended game window on local desktop (tracked via lightweight 1 s check on `session_tick`). |
| **Game exits externally while suspended** | `GetProcessTimes` check sees PID gone or CreationTime changed; `session_tick` clears stale `suspended_pids` entry cleanly. |
| **Anti-cheat or launcher child spawns right before suspend** | Tree walk detects denylisted executable -> refuses entire suspend atomically; zero threads frozen. |

### 4.3 Canonical Write-Ahead Crash-Restore Journal (`RecoveryJournalV2`) & Multi-Layer Watchdog

Because `rust/host/stream/src/session_control.rs:174` writes `session.json` asynchronously *without* `sync_all()`, Quick Suspend **never relies solely on `session.json`'s async writer**. Instead, Item 08 and Item 17 (`17-ux-craft-and-resilience.md`) share **ONE canonical `RecoveryJournalV2`** (`rust/core/session/src/recovery_journal.rs`) and **ONE canonical service recovery CLI hook (`--recover-session`)** — zero duplicate or conflicting journal files or CLI flags:

1. **Canonical Write-Ahead Journal (`<profile>\recovery-journal.json`, alongside `session.json`):**
   - Schema (`RecoveryJournalV2`, `schema_version: 2`, defined in `rust/core/session/src/recovery_journal.rs`), carrying `suspended_pids` plus display/DPI/audio recovery state:
     ```json
     {
       "schema_version": 2,
       "host_pid": 14820,
       "host_started": 133765123456789012,
       "display_entries": {},
       "physical_dpi_baseline": {},
       "audio_state": null,
       "suspended_pids": [
         {
           "pid": 22104,
           "created_100ns": 133765119000123456,
           "app_id": 1042,
           "app_title": "Hollow Knight",
           "exe": "hollow_knight.exe",
           "suspend_count": 1,
           "prev_audio_muted": false,
           "suspended_unix_ms": 1763772200000
         }
       ]
     }
     ```
   - Written synchronously via `recovery-journal.json.tmp` -> `File::sync_all()` -> atomic `std::fs::rename` **BEFORE** the first `NtSuspendProcess` call.
   - `suspended_pids` entries are cleared **only after** `NtResumeProcess` has completed (or verified exited) for every entry; `recovery-journal.json` is deleted once `suspended_pids` and all display/DPI/audio entries are empty.
2. **Unified visibility in `session.json`:** `butterpollo_core::session_state::Journal` gains `#[serde(default)] pub game_frozen: bool` so interrupted-session history notes that the game was frozen and recovered.
3. **Three-layer crash recovery (zero permanently-frozen games):**
   - **Layer 1 — In-process `Drop` & panic hook (`QuickSuspendTracker::drop` + `session_shutdown`):** Clean shutdown or unwinding panic immediately calls `resume_all()`.
   - **Layer 2 — Host startup recovery (`recover_session` in `rust/host/src/main.rs`):** Runs unconditionally at startup (both supervised and standalone) BEFORE any HTTP/RTSP listener starts. Reads `RecoveryJournalV2` from `recovery-journal.json`, verifies `(pid, created_100ns)` for every entry in `suspended_pids`, calls `NtResumeProcess` (`suspend_count` times), restores WASAPI mute, clears `suspended_pids` (deleting `recovery-journal.json` when no display/DPI/audio state remains), and logs a visible `quick_suspend_recovered` warning: `"Rubylight stopped while <Title> was suspended; the game was automatically resumed."`
   - **Layer 3 — Windows Service Watchdog (`butterpollo-service.exe` + `rust/host/stream/src/service_tasks.rs`):**
     - Defines **ONE canonical service recovery CLI hook** `--recover-session --config-dir <PROFILE>` in `service_tasks.rs` (same contract as `--restore-nucleus-at-console`): Item 08 defines `--recover-session` to thaw `suspended_pids` and restore session state from `recovery-journal.json`, and Item 17 (`17-ux-craft-and-resilience.md`) hardens this exact same hook with per-monitor DPI settlement and a 10-cycle forced-kill crash-restore soak.
     - If the supervised `butterpollo.exe` exits with a non-zero crash code or is killed while `<PROFILE>\recovery-journal.json` exists, `rust/service/src/supervisor.rs` immediately runs `butterpollo.exe --recover-session --config-dir <PROFILE>` even if restart backoff is active or the service is stopping!
     - **Service versioning compliance (`rust/release/README.md`):** Because Layer 3 modifies `rust/service/src/supervisor.rs` and `rust/service/src/tasks.rs`, its implementer slice explicitly bumps `rust/service/Cargo.toml` and runs `python rust/release/service_inputs.py --update` (verified by `test_release.py`).

---

## 5. Settings & 10/10 Craft Plan

### 5.1 Fewer-Settings Rule & Default-Off Opt-In

- **One setting only** (`quick_suspend`), available in Host settings and per-app overrides (`override_allowed`, app editor in Console 2.0):
  - Values: `off` (**default**), `manual` (Suspend Game button in Stream Menu / Native API only), `on_disconnect` (manual button + auto-freeze 3 s after disconnecting).
  - **2-sentence help text (Console 2.0 + Nightjar):**
    - `"Pauses eligible single-player games when you suspend or disconnect so your PC stays cool and quiet until you resume. Online games and anti-cheat titles are never paused; leave off for multiplayer games or titles that disconnect when paused."`
- **Zero local Nightjar preference keys** (`SettingsKeysTest` untouched).
- **No "Disable anti-cheat check" toggle:** The safety denylist is strictly non-overridable — fewer settings AND zero foot-guns.

### 5.2 10/10 Craft & Policy Compliance

- **Zero `"Experimental"` in UI:** Never label Quick Suspend or the Suspend Game tile as "Experimental".
- **Zero mod names in UI/Nightjar:** Strictly host-neutral wording ("Quick Suspend", "Suspend Game", "Resume Game").
- **Zero stream-time blur:** Stream Menu tile and Console Home "Suspended" badge use clean elevated cards with zero backdrop blur over live video.
- **Dynamic input glyphs:** Nightjar Stream Menu **Suspend Game** tile and Home **Resume** prompt show the active controller, remote, or keyboard glyph (touch and mouse show no button hints per `client/design/README.md`).
- **Plain-English error & refusal states:**
  - `DisabledForApp` -> `"Quick Suspend is off for this game. Enable it in this game's Rubylight settings to pause it between sessions."`
  - `AntiCheatDetected` -> `"This game uses anti-cheat protection and cannot be paused safely."`
  - `ProtectedProcess` -> `"Windows blocked pausing this game's process, so it was left running."`
  - `NoOwnedProcess` -> `"Quick Suspend works only for launched games, not Desktop sessions."`
  - `GuestsActive` -> `"Cannot suspend the game while guests are watching or playing."`
  - `CoopSession` -> `"Quick Suspend is unavailable during multi-instance co-op sessions."`
- **All 15 locales:** Every label, badge, help text, and refusal string translated across all 15 locales (`en`, `de`, `es`, `fr`, `it`, `ja`, `ko`, `pl`, `pt_br`, `ru`, `tr`, `uk`, `vi`, `zh_hans`, `zh_hant`). Desktop routes all 14 non-English translation tables to `client/protocol/desktop-config/src/strings_70/<code>.rs` (English literals live at call sites via `tr`/`trf`/`en`; never ad-hoc files and never growing grandfathered `strings/*.rs` past 1,750 LOC).

---

## 6. Disjoint Implementer Slices

All slices have **strictly non-overlapping file lists** for Phase 3 `parallel()`. Note that **Slice 8E owns ONLY `rust/web/**` frontend files** — all Rust host web/console/native_api handlers live in **Slice 8D** so parallel slices never have intra-crate Rust compile dependencies.

| Slice | Parallel Group | Exclusive File List (zero overlap!) |
|---|---|---|
| **8A: Protocol `0x554B` & Native API Wire Types** | Stage 1 | `client/protocol/core/src/ids.rs`, `client/protocol/core/src/lib.rs`, `client/protocol/core/src/quick_suspend.rs` (new), `client/protocol/core/src/fuzz.rs`, `client/protocol/native-api/src/lib.rs`, `client/protocol/native-api/src/quick_suspend.rs` (new), `client/protocol/native-api/src/session.rs`, `client/protocol/native-api/tests/fixtures.rs`, `client/protocol/native-api/tests/fixtures/quick_suspend_*.json` (new), `docs/control-messages.md`, `docs/native-api.md` |
| **8B: Core Safety Policy, Denylist & Canonical `RecoveryJournalV2` Schema** | Stage 1 (parallel with 8A) | `rust/core/src/lib.rs`, `rust/core/session/src/recovery_journal.rs` (new), `rust/core/session/src/quick_suspend.rs` (new), `rust/core/session/src/session_state.rs`, `rust/core/config/src/config.rs`, `rust/core/config/src/config/overrides.rs`, `rust/core/config/src/config/tests.rs` |
| **8C: Windows NT Suspend/Resume & Per-PID Audio Mute** | Stage 1 (parallel with 8A+8B) | `rust/windows/io/src/process.rs`, `rust/windows/io/src/process/suspend.rs` (new), `rust/windows/src/lib.rs`, `rust/windows/io/src/audio_mute.rs` (new) |
| **8D: Host Session Orchestration, Control, Native API, Console Settings & Watchdog** | Stage 2 (after 8A+8B+8C) | `rust/core/media-policy/src/extensions.rs`, `rust/host/stream/src/extensions.rs`, `rust/host/stream/src/process.rs`, `rust/host/stream/src/state.rs`, `rust/host/stream/src/session_control.rs`, `rust/host/stream/src/session_control/quick_suspend.rs` (new), `rust/host/stream/src/nvhttp/launch_steps.rs`, `rust/host/stream/src/stream/control.rs`, `rust/host/web/src/native_api.rs`, `rust/host/web/src/native_api/quick_suspend.rs` (new), `rust/host/web/src/console/settings.rs`, `rust/host/stream/src/service_tasks.rs`, `rust/host/src/main.rs`, `rust/service/Cargo.toml`, `rust/service/src/supervisor.rs`, `rust/service/src/tasks.rs`, `rust/service/inputs.txt`, `rust/assets/locale/*.json`, `docs/configuration.md`, `tests/host/tests/nvhttp_parity/native_tests/quick_suspend.rs` (new), `tests/host/tests/reconnect_tests/quick_suspend_tests.rs` (new) |
| **8E: Console 2.0 Frontend (`rust/web/**` ONLY)** | Stage 2 (parallel with 8D) | `rust/web/src/lib/schema/basics.ts`, `rust/web/src/components/session/SuspendedGameBanner.svelte` (new), `rust/web/QuickSuspend.test.mjs` (new) |
| **8F: Nightjar Core, Desktop & Android** | Stage 2 (parallel with 8D+8E) | `client/protocol/policy/src/http/capabilities.rs`, `client/protocol/client/src/http/native.rs`, `client/protocol/client/src/http/native_suspend.rs` (new), `client/protocol/client/src/control/messages.rs`, `client/protocol/client/src/control/events.rs`, `client/protocol/client/src/control/session.rs`, `client/protocol/client/tests/control_messages_contract.rs`, `client/protocol/client/tests/capabilities_contract.rs`, `client/protocol/desktop-media/src/stream/mod.rs`, `client/protocol/desktop-media/src/stream/quick_suspend.rs` (new), `client/protocol/desktop-config/src/strings.rs`, `client/protocol/desktop-config/src/strings_70/<code>.rs` (14 non-English locale tables; never ad-hoc files or grandfathered `strings/*.rs`), `client/protocol/android/src/lib.rs`, `client/app/src/main/java/com/limelight/nvstream/http/HostCapabilities.java`, `client/app/src/main/java/com/limelight/nvstream/jni/RustBridge.java`, `client/app/src/main/java/com/limelight/ui/StreamMenu.java`, `client/app/src/main/java/com/limelight/ui/StreamMenuSuspendSection.java` (new), `client/app/src/main/res/values*/strings.xml`, `client/app/src/test/resources/protocol/capabilities/*` |

*(Every path above is marked `docs/code-map.md` for Phase 2 reconciliation.)*

---

## 7. Verification & Measurement Plan

### 7.1 Narrowest Unit, Safety & Crash-Recovery Tests (No GPU Required)

- **Protocol & fuzz tests:**
  - `cargo test -p rubylight-protocol --locked quick_suspend ids` and `PROPTEST_CASES=2000 cargo test -p rubylight-protocol --locked fuzz`.
- **Core safety, denylist & journal tests (`cargo test -p butterpollo-core --locked quick_suspend recovery_journal`):**
  - Every anti-cheat binary/service pattern (`EasyAntiCheat_EOS.exe`, `BEService.exe`, `vgc.exe`, `EAAntiCheat.GameService.exe`, etc.) returns `Err(AntiCheatDetected)` regardless of casing or path.
  - System/critical processes (`dwm.exe`, `csrss.exe`, `butterpollo.exe`, PID 4, Session 0) return `Err(DenylistedProcess)`.
  - Launcher filtering removes `steam.exe`/`steamwebhelper.exe`/`EpicGamesLauncher.exe` while keeping game child PIDs; launcher-only app returns `Err(NoOwnedProcess)`.
  - Leaf-first topological ordering freezes children before parent and resumes parent before children.
  - `RecoveryJournalV2` (`recovery-journal.json`) serde round-trip (`suspended_pids` + display/DPI/audio state) + malformed/truncated journal handling.
- **Windows process suspend/resume integration tests (`cargo test -p butterpollo-windows --locked process::suspend`):**
  - Spawns a harmless dummy child process (`ping` / test helper subprocess) emitting heartbeat ticks to a pipe:
    1. Verifies `suspend_tree` halts heartbeat ticks within 50 ms.
    2. Verifies **PID-reuse safety**: passing a mismatched `started` (`CreationTime`) refuses to touch the running child.
    3. Verifies **atomic rollback**: injecting a simulated failure on child #2 immediately resumes child #1.
    4. Verifies `resume_tree` restores heartbeat ticks within 50 ms.
- **Host session, service watchdog & interop tests:**
  - `cargo test -p butterpollo --locked quick_suspend`:
    - Verifies `recovery-journal.json` (`RecoveryJournalV2`) exists on disk *before* mock `NtSuspendProcess` runs (write-ahead ordering assertion!).
    - Simulates host crash (leaves `recovery-journal.json` with `suspended_pids` behind without calling `Drop`) -> runs `recover_session` and `--recover-session` -> asserts dummy process is resumed, `suspended_pids` cleared (`recovery-journal.json` deleted when empty), and `quick_suspend_recovered` warning recorded.
    - Verifies End session, Close game, `/resume`, and grace-period expiry all resume frozen processes before teardown/launch.
    - Verifies stock Moonlight clients unaffected, guests refused (`GuestsActive`/`NotOwner`), and `test_release.py` passes with updated `rust/service/inputs.txt`.
  - `cargo test -p butterpollo --locked interop_tests` (Nightjar engine <-> host `0x554B` & `/api/native/v1/session/suspend`).
- **Nightjar & arch checks:**
  - `cargo test -p nightjar-core --locked capabilities_contract control_messages_contract`
  - `gradlew :app:testNonRootDebugUnitTest --tests com.limelight.ui.StreamMenuTest --tests com.limelight.preferences.SettingsKeysTest`
  - `python tools/arch_check.py` (explicitly verifying `rust/host/stream/src/session_control.rs <= 800` LOC, grandfathered `client/protocol/desktop-config/src/strings/*.rs <= 1,750` LOC, and all new modules `<= 800` LOC with `//!`/`/**` headers!)
  - `python tools/code_map.py check`

### 7.2 Visual & Screenshot Checks

- **Console 2.0:** `QuickSuspend.test.mjs`, `SettingsSchema.test.mjs`, and `rust/web/scripts/screenshots.cjs` showing the app-editor Quick Suspend setting and active "Game Suspended — Resume" banner.
- **Nightjar Desktop & Android:** `shot.rs` and `StreamMenuTest` verifying the Stream Menu **Suspend Game** tile (Ready, Refused/Anti-Cheat disabled state, zero stream-time blur, dynamic controller glyphs) and Console Home **"Suspended — Resume"** badge.

### 7.3 Serialized Idle-Gated Hardware Criteria

Executed on **RX 7900 XT desktop** and **Radeon 780M laptop** with an isolated test host (`--port 48123 --bind 127.0.0.1`), after verifying `/serverinfo` `RustHostSessionCount == 0` and `RustHostPendingSessionCount == 0`, `GPU-LOCK` held, and quiet CPU:

1. **Real D3D11/D3D12 game-loop suspend/resume test (using `motion_probe.exe` / synthetic DX11+DX12 render loop app opted into `quick_suspend = manual`):**
   - Stream starts (`e2e.py`), runs 15 s -> `POST /api/native/v1/session/suspend` (`disconnect: true`) -> verify:
     - Process tree suspends within `<= 250 ms` and GPU/CPU usage of the probe drops to `0%`.
     - Host enters `State::Suspended` (`game_frozen: true`), capture & encoder terminate cleanly, and display topology stays intact.
     - Wait 30 s -> client reconnects (`/resume`) -> verify process resumes within `<= 250 ms`, zero `DXGI_ERROR_DEVICE_REMOVED`, stream resumes within `<= 1.5 s` with normal frame pacing.
2. **Forced host kill recovery test (isolated test host ONLY, never installed service):**
   - Suspend `motion_probe.exe` -> `Stop-Process -Force` on isolated test `butterpollo.exe` -> run `butterpollo.exe --recover-session --config-dir C:\tests\wave2-suspend` -> verify `motion_probe.exe` immediately resumes rendering and `suspended_pids` / `recovery-journal.json` are cleared cleanly (hardened and 10-cycle forced-kill soaked in `17-ux-craft-and-resilience.md`).
3. **Default-path latency budget:** When Quick Suspend is `off` (default) or armed-but-not-suspended during live streaming, **zero code runs on capture/encode/video-send/audio threads** (`+0.00 ms` across convert p99, encode p99, and `present_to_sent` p99; `0` `slice_delivery_overflows`).
4. **NVIDIA note:** Quick Suspend uses OS-level NT process suspension and WASAPI mute, independent of GPU vendor; NVENC stream teardown/resume on reconnect uses existing NVENC paths (checked against vendored headers and `nvenc/tests.rs`, never claimed as run on NVIDIA hardware).
5. **Exit code 3 abort rule:** If any `e2e.py` or `check.ps1` leg exits with code **`3`** (AMD encoder hang), **STOP immediately**, preserve `result.json` (`gpu_hang`), and investigate — **NEVER retry**.
