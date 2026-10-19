# Android client parity log (historical)

> Historical record, written while this app was called Butterpollo and then Rubylight Android.
> In client context "Rubylight" below means this app, now Nightjar; host links and host behavior
> refer to the Rubylight host. See [RENAME.md](../docs/RENAME.md).

> The current one-page status against the host's `main` is [PARITY-MATRIX.md](../docs/PARITY-MATRIX.md).
> This file keeps the detailed evidence and the real-device checklist.

Client usability follow-up on 2026-10-09 adds Android 15 Surface cadence hints and tests the
existing Android 16 ARR path, separates PyroWave overlay signals, uses completed decode time for
ordinary-codec adaptation, improves multi-interface Wake-on-LAN, and adds settings search and PiP
Disconnect. Digital remapping now also applies to USB controller reports. Numpad/Print Screen and
supplementary Unicode regressions are fixed. Remaining explicit dialogs, progress indicators,
controller mapping and Help navigation follow the existing Material 3 theme. No host files changed.

Client review on 2026-10-09: this branch already contained record framing, a USB DualSense driver
and role-aware sessions. This follow-up adds decode time to PyroWave rate control, a separate
off-by-default DualSense toggle with Material 3 permission explanation, captured USB input fixtures,
Material 3 launch confirmation and interruption/unknown-410 regressions. The current checklist below
supersedes older milestone instructions about absent records, triggers or role support.
The read-only local host reference was [118a0abb](https://github.com/RamazanKara/Rubylight/tree/118a0abb18480ff1da5564c57a2e00049ed67e47);
only this client changed.

Re-audited on 2026-10-08 against the read-only host checkout at
[Rubylight `de2d920127bc1113ff41ebc0d313a78eaf8b0e1b`](https://github.com/RamazanKara/Rubylight/tree/de2d920127bc1113ff41ebc0d313a78eaf8b0e1b).
Remote HEAD was [rc.22 `3e264545e75d5da238b150dfca214825f7a4a1ec`](https://github.com/RamazanKara/Rubylight/tree/3e264545e75d5da238b150dfca214825f7a4a1ec);
the [comparison](https://github.com/RamazanKara/Rubylight/compare/de2d920127bc1113ff41ebc0d313a78eaf8b0e1b...3e264545e75d5da238b150dfca214825f7a4a1ec)
contains release/version documentation only, with no protocol changes. The host checkout was not changed.
The current Rust host is authoritative; the retained C++ implementation is historical.
“Supported” below means implemented protocol paths, not a real-device streaming certification.

| Capability | Host support | Client status |
| --- | --- | --- |
| Discovery, addresses, ports, wake | mDNS, IPv4/IPv6, `hostname`, `LocalIP`, `mac`, `HttpsPort`, `ExternalPort` | Existing discovery, manual addresses and port handling. Wake-on-LAN validates the MAC, tries every active IPv4 interface broadcast plus global broadcast/resolved addresses, deduplicates destinations, bounds alternate ports and sends three packets per destination despite partial failures. |
| Pairing and device identity | PIN/certificate pairing, `PairStatus`, host `uniqueid`; device UUID bound to certificate | Persistent installation ID, Android model name, certificate pinning and authenticated HTTPS unpair. Installation ID, private key and client certificate are excluded from backup/device transfer so a restored installation pairs independently. Same-install updates preserve identity. |
| One-time PIN, device permissions | OTP creation via admin API, `otpauth` pairing; `Permission`, per-device enable and access masks | Host context menu accepts the generated four-digit PIN and passphrase; masked, transient fields and a timed pairing request. Ordinary PIN pairing remains available. Unsigned permission masks are displayed and refreshed before host actions; missing fields preserve stock behavior, malformed masks grant nothing. |
| App list and artwork | `/applist`, `/appasset`; `AppTitle`, `ID`, `IsHdrSupported`, `UUID`, `IDX`, `ArtVersion`, permission-filtered entries | UUID-aware launch/resume and shortcuts, host index ordering with alphabetical fallback, live metadata updates, and artwork version/identity keys for memory, disk, pending loads and TV posters. Unknown fields remain compatible. Unchanged host artwork tokens cannot signal changed covers. |
| Launch, resume, quit and ownership | `/launch`, `/resume`, `/cancel`, `currentgame`, `currentgameuuid`, `state`, `gamesession`, `sessionUrl0` | Local disconnect preserves the app; reconnect waits for teardown. UUID matching, refreshed Resume ownership, role-scoped disconnect and explicit replacement/Terminate confirmation are implemented. View or launch permission allows viewing; unknown 410 messages never cause retries. |
| Per-host, per-app and per-device settings | Client launch parameters; host merges stream/input/display/encoder overrides, prep/undo commands, `APOLLO_*` environment | Local per-host stream profiles now save custom dimensions, refresh, bitrate, render scale, virtual display, HDR/range/chroma and codec choice. Host app/device policy applies normally. Editing the host's profiles, permissions or app settings requires its separate authenticated administration API; no native admin editor. |
| Per-device virtual display | `VirtualDisplayCapable`, `VirtualDisplayDriverReady`, `virtualDisplay=1`; certificate-derived stable display identity | New opt-in request, sent only when advertised and driver not reported unavailable; otherwise a notice and host policy fallback. Shared/off/layout/forced-output policy stays host controlled. |
| Native resolution and render scale | `mode=WIDTHxHEIGHTxRATE`, SDP viewport, `scaleFactor` | Native portrait/landscape/fullscreen choices and per-host custom dimensions. Per-host render scale accepts 50–200%, default 100%, with the virtual-display request. Stream viewport is unchanged by render scale. |
| High/fractional refresh | Integer/decimal launch rate, SDP `maxFPS`, `clientRefreshRateX100` | Per-host explicit refresh to 0.01 Hz is sent to Rubylight. Without an override, matching panel refresh still supplies fractional precision. Sunshine/Apollo retain integer launch syntax; configured high rates remain available subject to device capability. |
| Desktop DPI, layout, mode overrides | Host `dd_virtual_display_scale`, per-device `display_mode`, layouts, HDR profiles, retained displays | Host configured; no client DPI field. `scaleFactor` scales render pixels, **not** Android density or Windows text size. Host/app overrides can supersede requested geometry/rate. |
| H.264 / HEVC / AV1 SDR | `ServerCodecModeSupport`, RTSP codec markers | Existing hardware MediaCodec paths and software-decoder exclusions retained. Stock Sunshine/Apollo/GFE fallback retained. |
| HEVC / AV1 HDR10 | Main10 flags, launch `hdrMode`, SDP `dynamicRangeMode`; ten-bit BT.2020/PQ | Negotiates a common HDR codec, preferring HDR over a higher-priority SDR codec. Requires HDR10 display and HDR-capable decoder. HDR10+ decoder profiles also qualify for static HDR10; no dynamic HDR10+ claim. Auto can discover AV1 for HDR. |
| Display/HDR metadata | Control type `0x010e`: enabled plus little-endian RGB/white primaries, mastering luminance, MaxCLL/MaxFALL, optional full-frame luminance | Converts to Android's 25-byte `KEY_HDR_STATIC_INFO`, preserves units, omits unsupported trailing full-frame field. Explicit BT.2020/PQ hints, HDR/SDR transitions including absent metadata, SurfaceView output; malformed short metadata ignored. |
| Ten-bit SDR | Host `prefer_sdr_10bit` changes an HDR-capable stream to SDR and sends HDR-off | Existing ten-bit decoding plus corrected HDR-off handling. No separately advertised client ten-bit-SDR request; host policy supplies it. |
| Standard YUV 4:4:4 | Sunshine codec bits `0x40000`–`0x400000`, SDP `x-ss-video[0].chromaSamplingType`; encoder dependent | New opt-in AVC High 4:4:4 8-bit / HEVC Main444 8-bit (API 37 profile) on decoders advertising those profiles and supporting the requested size/rate. Raw AVC 4:4:4 SPS preserved. Negotiation falls back to 4:2:0 with a notice and actual chroma in the overlay. HDR takes priority. |
| HEVC/AV1 ten-bit 4:4:4, AV1 eight-bit 4:4:4 | Encoder-dependent standard flags; Radeon principally offers HDR 4:4:4 through PyroWave | No standard Android profile identifiers for these combinations in SDK 37; never inferred from Main10 or output color formats. Uses supported 4:2:0 instead. PyroWave can supply these combinations on qualified Vulkan devices; see milestone 4b below. |
| Host frame timing | Video short header carries unsigned processing duration in 100 µs units; zero means unavailable | Now beside client latency as avg/p95/p99 over 600 observed frames, also `host_processing_ms` in per-frame CSV. Host claim-to-packet duration is not encode-only or end-to-end latency. No clock subtraction. |
| Capture/encode/frame-age breakdown | Detailed timing in host diagnostics/web API; not separately carried in standard video packets | Not fabricated from processing duration; admin telemetry UI deferred. |
| Audio | Opus stereo, 5.1, 7.1, quality/channel-map negotiation, local playback, encryption | Existing audio negotiation/playback. Endpoint selection remains host configured. |
| Transport and recovery | RTSP/encrypted RTSP, UDP video/audio, reliable encrypted control, FEC, keyframe recovery, optional reference invalidation, ping/connect-data extensions, QoS | Pinned moonlight-common-c `f900dd47` handles these. Slice/reference negotiation and milestone 2 latency controls retained. |
| Keyboard/mouse/touch/pen | Standard input plus `x-ss-general.featureFlags` pen/touch bit | Hardware/soft keyboards, captured/absolute mouse and stylus retained. Stream menu selects trackpad, direct touch-as-mouse or native multi-touch with capability fallback; native gestures send cancel-all on focus/mode changes. Keyboard/OSC cleanup and explicit soft-keyboard access added. |
| Controllers and feedback | Multiple pads, touch bit, motion, battery, rumble/triggers, LED; driver-dependent DualSense effects | Android/vendor input and per-model digital remapping remain. The opt-in USB DualSense/Edge driver handles two touch contacts, calibrated motion, battery, rumble, LEDs, raw host trigger effects and Edge back paddles. Saved model-specific digital mappings also apply to USB reports; Bluetooth adaptive triggers remain outside this path. |
| Frame limiter and VRR | `FrameLimiterSupported`, `FrameLimiterEnabled`, `VirtualDisplayFrameLimiterEnabled`, `FrameLimiterFpsLimitMilliHz`; `vrr` aliases and SDP `vrrLowLatency`, 1000 Hz virtual mode | Stream menu reports manual/virtual limiter state and fractional configured cap (zero means stream rate). Host defaults and selected Android pacing apply. Opt-in **Variable refresh (VRR)** sends `vrr=1` (not to GFE), which turns on the host's VRR pacing; the client uses minimum-latency pacing with measured-cadence hints on Android 16 ARR displays and Android 15 SurfaceView. Older platforms, Android 15 TextureView and Android 16 without ARR retain fixed-rate selection. |
| Runtime bitrate / ABR | `/bitrate`, `/api/abr/capabilities`: client-driven runtime bitrate, `supported:false` for host ABR | Opt-in per-host control using loss and completed decode budgets for ordinary codecs; separate PyroWave control also uses queueing. Both require decode headroom for recovery and reset healthy streaks during menus/host actions. Manual changes/request failure disable adaptation. Stock hosts retain startup behavior; no automatic codec/resolution switch. |
| Remote monitor/input and control tiles | Synthetic app IDs/UUIDs, `remote_monitor`, `input_only`, resume/disconnect/terminate/replace actions | Monitor input is blocked; Input-only skips media threads/decoders/watchdogs. Scoped disconnect, ownership refresh and Material 3 confirmation handle recognized XML status 410 replies (HTTP transport status 200). Device lifecycle checks remain required. |
| Clipboard and server commands | Permission-scoped text `/actions/clipboard`, advertised `ServerCommand` names | Explicit foreground text send/receive over paired HTTPS with direction permissions, UTF-8, 1 MiB limit and no content logging. Commands use reliable encrypted control type `0x3000`, original 8-bit index and host-specific payload size; confirmation, permission refresh and one-second spacing. Host enforces app restrictions; no execution acknowledgment exists. |
| PyroWave | Codec/SDP flags, bitstream ID, adaptive FEC/records, `PyroWaveHostLinkMbps`, bandwidth probe bytes/endpoint | Explicit codec choice, Vulkan HDR/4:4:4, runtime/warm-up gates and bandwidth probe. Compatible Rust versions negotiate adaptive records via pyrowaveFeatures=1; unknown versions retain ordinary containers and a bitstream mismatch falls back to conventional codecs. Startup accepts 2 Gbps; runtime changes cap at 500 Mbps. Phone certification remains open. |
| Host diagnostics/version | `appversion`, `GfeVersion`, `MaxLumaPixelsHEVC`, `RustHostVersion`; loopback-only session/pending/app/profile fields | Accepts the current host's `7.1.431.-1` trailing build sentinel while rejecting other malformed negatives. Rust version gates decimal launch rates. Loopback-only diagnostics ignored safely. |
| Android integration and overlay | Android launcher/PiP and existing stream telemetry | Pinned app shortcuts and TV channels retained. PiP adds local Disconnect and bounds custom aspect ratios. Settings search uses device-filtered titles, descriptions and categories, then opens the original setting. Overlay toggles immediately from the stream menu or Ctrl+Alt+Shift+S. Shortcut intents carry app UUID; native touch uses the menu for soft-keyboard access. |
| Library and host administration | Steam/Playnite sync, Lossless Scaling, RTSS/RTX HDR/TrueHDR, display/HDR profiles, settings, devices, logs, updates, auth/tokens | Host-side effects work with ordinary launches; administration remains in the web console, not a streaming-client parity requirement. No admin credentials added to pairing. |

Source: host [protocol handlers](https://github.com/RamazanKara/Rubylight/blob/3e264545e75d5da238b150dfca214825f7a4a1ec/rust/host/src/nvhttp.rs),
[RTSP](https://github.com/RamazanKara/Rubylight/blob/3e264545e75d5da238b150dfca214825f7a4a1ec/rust/core/src/rtsp.rs),
[display policy](https://github.com/RamazanKara/Rubylight/blob/3e264545e75d5da238b150dfca214825f7a4a1ec/rust/host/src/display_session.rs),
[HDR layout](https://github.com/RamazanKara/Rubylight/blob/3e264545e75d5da238b150dfca214825f7a4a1ec/rust/core/src/hdr.rs),
[compatibility audit](https://github.com/RamazanKara/Rubylight/blob/3e264545e75d5da238b150dfca214825f7a4a1ec/rust/PARITY.md),
and [admin API](https://github.com/RamazanKara/Rubylight/blob/3e264545e75d5da238b150dfca214825f7a4a1ec/docs/api.md).
Android limits: [codec profiles](https://developer.android.com/reference/android/media/MediaCodecInfo.CodecProfileLevel)
and [MediaFormat HDR/color keys](https://developer.android.com/reference/android/media/MediaFormat).

## Milestone 7: client features and current limits

This round prioritizes per-host stream setup, usable in-stream controls, recovery, network adaptation
and the advertised pairing/library extensions. It does not establish complete Rubylight parity.
Capabilities in the table describe source implementation; the real-device checks below are still required.

### Comparison with the requested clients

The comparison was checked against the projects' primary documentation on 2026-10-08. Their feature
lists describe their own products; the Rubylight column is based on this repository's code.

| Reference | Relevant baseline | Rubylight Android status |
| --- | --- | --- |
| [Moonlight Android input guide](https://github.com/moonlight-stream/moonlight-docs/wiki/Setup-Guide#keyboardmousegamepad-input-options) and [source](https://github.com/moonlight-stream/moonlight-android) | Android controller mappings, direct/trackpad mouse emulation, external keyboards/mice and shortcut controls. | These inherited paths remain. Multiple pads use the existing 16-slot client mask, subject to host/driver limits; no claim that 16 physical pads were tested. Existing Wi-Fi locks, Wake-on-LAN, pinned shortcuts, TV channels, OSC layout storage and PiP remain. The stream menu now makes touch mode, OSC editing, keyboard, overlay and reconnect easy to reach. |
| [Artemis Android feature list](https://github.com/ClassicOldSong/moonlight-android#features) | Custom resolutions/bitrates, mouse-mode switching, a game menu, virtual displays, server commands and clipboard; additional custom virtual buttons, external-monitor and desktop-use tools. | Per-host dimensions/refresh/bitrate, native touch, a local menu, virtual-display requests and existing manual clipboard/server commands cover the principal streaming overlaps. Per-game keyboard/mouse overlay buttons now support portable JSON, held/toggle actions and sharing. Tablet pan/zoom and external video with a local touchpad are implemented; SBS 3D remains absent. |
| [Moonlight-qt feature list](https://github.com/moonlight-stream/moonlight-qt#features) | H.264/HEVC/AV1, HDR/4:4:4, surround sound, multi-touch, gamepad feedback/motion, captured/direct mouse and system shortcuts. | Ordinary codecs/HDR/audio/input follow Android's decoder, display and input capabilities. Native multi-touch is enabled with a host capability fallback. Ctrl+Alt+Shift+S toggles the performance overlay. Android's supported standard 4:4:4 profiles remain narrower than desktop decoders; unsupported formats fall back. |

The inherited controller mappings include Android-standard and vendor-specific mappings, face-button
swap, deadzone controls and multi-controller mode. **Settings → Controller buttons** remaps digital
buttons per controller model (USB vendor/product ID, or name when IDs are missing): press a button,
then pick A/B/X/Y, bumpers, stick clicks, Start, Select, Guide or nothing. A mapping replaces
face-button flipping for that button and applies from the next stream. D-pad directions, analog trigger swaps/disable and stick-axis source/inversion mappings are configured separately under Sticks, triggers and D-pad. USB Xbox/DualSense/Edge reports now apply the same saved model mappings;
explicit bindings override face swapping, mappings do not chain, and a shared target stays held until
all sources release. Configure the model in Settings before streaming. OSC editing moves/resizes the existing gamepad. Separate per-game keyboard/mouse overlays create held or toggle buttons, with validated JSON import/export; see docs/controls.md. Native multi-touch forwards finger pointers; the stream menu
opens the soft keyboard so three fingers remain available to the host. Mouse-emulation modes retain
their existing three-finger keyboard gesture.

### What the new paths do

- Long-press a PC and open **Streaming settings for this PC** to save a local profile keyed by host UUID.
  Reset restores global preferences. Values cover width/height, refresh to two decimal places, bitrate,
  render scale, codec including PyroWave, virtual display, HDR, full range and 4:4:4. Saving a profile
  does not change the host's administrator settings. Hardware checks and host policy can still reject
  or override the request; accepted input ranges are not decoder/display capability guarantees.
- The host's explicit fractional rate wins over Android panel matching on Rubylight. Sunshine and
  Apollo continue receiving rounded integer launch rates. Render scale is sent with an available
  virtual-display request; it changes host render pixels, not Android density or desktop DPI.
- The local stream menu opens without waiting for host HTTP. Overlay, touch mode, keyboard, OSC,
  move/resize/save layout, reconnect and disconnect are available there. Reconnect stops and joins
  the old connection before starting a new activity/session; it does not quit the host application.
  Transport errors offer reconnect. Touch cancellation and input release prevent the new menu paths
  from leaving emulated input held; hardware/OEM behavior still needs the checks below.
- Automatic bitrate is opt-in and saved per host, available on Rubylight. For ordinary codecs it
  samples recent video/network loss, completed decode time and common-c poor-connection status once per second, waits
  through transient bad samples, reduces by 20% no faster than every five seconds, then increases
  slowly after 15 healthy seconds with at least 25% decode headroom. Missing decode measurements prevent
  recovery; a decoder over its frame budget can trigger reduction without network loss. It respects the host's applied cap, pauses decisions during menu
  interaction/other host actions, clears healthy streaks during that pause, and disables itself after a request failure or a manual bitrate
  change. PyroWave has separate saved opt-in state: two congested samples reduce by 15%, no faster
  than every three seconds; 20 healthy seconds allow a 2% rise (at least 250 kbps). Its signals include
  missing records, lost frames, queue delay and completed decode time versus the stream frame budget.
  Decode time excludes the subsequent presentation wait; missing decode measurements or less than
  25% decode headroom prevent recovery. Both controllers respect the applied host cap and 500 Mbps
  runtime limit. Neither changes resolution, frame rate or codec; startup settings remain the baseline.
- On a paired capable PC, **Test PyroWave bandwidth** asks before downloading 32 MiB over pinned HTTPS.
  It reports measured HTTP throughput and the advertised host link speed, has a 30-second total
  timeout, and cancels when leaving the screen. It does not alter stream settings. HTTP throughput
  includes request overhead and does not certify UDP loss, FEC headroom or sustained streaming.
- **Pair with one-time PIN** accepts the host-generated four-digit PIN and chosen passphrase.
  The host's three-minute expiry applies. The request sends SHA-256 of PIN + uppercase salt hex +
  UTF-8 passphrase as `otpauth`; the normal certificate challenge still authenticates the pair.
  The client does not save these fields or log pairing URLs. Generating an OTP and granting device
  permissions remain administrator tasks on the host.
- App UUIDs travel through launcher shortcuts and launch/resume requests. A refreshed app list
  resolves changed numeric IDs; running UUID matching and UUID-only host resolution preserve known
  shortcuts when List permission is unavailable. `IDX` orders apps when supplied, and artwork cache
  keys include app UUID and opaque `ArtVersion`. The current Rust host emits the numeric app ID as
  its artwork token: replacing a cover without changing that token remains a host-side signalling gap.
  New UUID-bearing shortcuts also use stable UUID keys, so later numeric-ID reuse cannot overwrite
  them. Recreate older numeric-only shortcuts from the current app list to gain this behavior;
  their original app identity cannot be safely inferred. Android TV recommendation program keys
  remain numeric, although launch intents carry UUIDs and poster URLs use versioned artwork keys.
- Android backup/device transfer excludes the client certificate, private key and installation ID.
  A restored installation therefore pairs again; a normal update of the same installation keeps its
  identity. Permission displays and action-time refresh remain independent of these local profiles.

### Remaining work and protocol boundaries

| Gap | Evidence and boundary |
| --- | --- |
| Virtual-display layouts, sharing/disable policy, DPI and retained displays | `display_session.rs` reads `virtual_display_mode`, `display_mode` and `virtual_display_layout` from paired-device extras, app extras or host configuration. Only `virtualDisplay`, `mode` and `scaleFactor` are client launch options here. Sending invented layout/DPI parameters would have no effect. Requesting no virtual display does not override a host/app forced display policy. Configure exclusive/extended/primary layouts and desktop scale in the host console. |
| Editing host or per-app/per-device settings and permissions | `/api/config`, `/api/apps`, `/api/clients/update`, `/api/clients/display-layout` and HDR-profile APIs require separate administrator authentication. Pairing certificates do not grant that access. Host overrides already apply during ordinary launches; a native admin UI/credential store is not implemented. |
| Detailed capture/encode/frame-age telemetry | The video short header carries one processing duration. Capture/encode breakdown, host warnings and broader diagnostics belong to authenticated host telemetry. The overlay/CSV reports available processing avg/p95/p99 and never infers encode-only or end-to-end time by subtracting unrelated clocks. A native admin telemetry view remains open. |
| VRR / ARR scanout | Client implemented and policy-tested: Android 16 detects ARR support; Android 15 SurfaceView uses seamless cadence hints without claiming detected support. Android 15 TextureView/older OS/non-ARR Android 16 use fixed-rate fallback. Actual refresh, touch boosts, power policy and presentation remain hardware checks. |
| PyroWave transport and 4K quality | Record framing now recovers surviving coefficients and conceals missing records with cached data; a datagram may affect several records or a split record. Framing and bitrate state machines have unit coverage, but packet loss, GPU performance and quality still need phone checks. Startup accepts 2000 Mbps; runtime /bitrate remains capped at 500 Mbps. Recommended-quality 4K60 may require more than gigabit Ethernet. |
| Remote Monitor, Remote Input and control-tile lifecycle | Implemented role-specific media/input gating, scoped disconnect and explicit confirmation. Replacement/Terminate XML 410 challenges require a user decision within 60 seconds, retry at most once and cancel on background/stop. Completion messages are distinguished from errors. Verify persistence/rejoin and two-client ownership on hardware. |
| Axis mapping and advanced feedback | The separate axis/D-pad editor maps normalized Android/USB reports while preserving analog range and held targets. USB-driver digital remapping is implemented and unit-tested. USB DualSense/Edge feedback is wired through common-c/JNI to physical USB reports, including opaque rc.23+ trigger payloads. Bluetooth adaptive triggers and USB audio haptics are not claimed. |
| Other Artemis desktop conveniences | Per-game keyboard/mouse buttons with JSON import/export, view pan/zoom and external-monitor video plus a local touchpad are implemented. Automatic clipboard sync and SBS 3D remain absent. Manual foreground text clipboard is intentional current behavior. No claim of every Artemis feature being reproduced. |
| Automatic bitrate on other hosts | Only the verified Rubylight runtime API is used; stock Sunshine/Apollo retain startup bitrate. PyroWave has its own loss/queue/decode policy. Automatic resolution/codec switching remains unimplemented. |

### Client follow-up evidence and UI audit

- ARR API boundaries follow the [Android ARR guide](https://developer.android.com/develop/ui/views/animations/adaptive-refresh-rate)
  and [Display capability API](https://developer.android.com/reference/android/view/Display#hasArrSupport()).
  Android 15 Surface hints request only seamless changes; the overlay says **cadence hints**, without
  claiming detected support. Android 16 checks ARR support for either rendering surface. Unit tests cover
  fixed-rate fallback, fractional rates, missing samples and votes capped at the requested stream rate.
- The PyroWave overlay separates **records missing**, **queue**, **completed decode**, and **GPU decode (last)**.
  Record loss is distinct from lost frames. Missing samples read **unavailable**; no render/scanout or
  end-to-end measurement is invented. JUnit covers formatting and completed-frame sampling/reset.
- Ordinary-codec adaptation reduces for sustained decode-budget overruns and permits recovery only with
  measured decode headroom. Both controllers restart healthy observation after menus/host actions suspend
  sampling; tests cover thresholds, unavailable data, cooldowns, floors, runtime caps and pause/recovery.
- Wake-on-LAN tests use a recording datagram socket and synthetic subnet destinations, with no host
  traffic. They cover magic-packet bytes, MAC separators/malformed values, duplicate targets, port overflow,
  failed routes and transient send failure. Interface enumeration and actual routing remain Android checks.
  Directed broadcasts reach attached subnets, not arbitrary networks through routers.
- Search matching is literal, case/accent insensitive and multi-word. Only preferences retained by the
  existing device checks are indexed. Results navigate to the original setting and scroll it into view,
  retaining dependencies and persistence. Query text survives recreation; Back clears search first.
  Matching is unit-tested; Android focus/lifecycle is not rendered.
- PiP Disconnect uses a per-activity action, immutable package-scoped PendingIntent and a receiver that
  is not exported. It only finishes an active media session while in PiP, preserving the host app.
  Ultrawide/portrait bounds follow the [Android PiP builder](https://developer.android.com/reference/android/app/PictureInPictureParams.Builder#setAspectRatio(android.util.Rational)).
  Ratio and role/lifecycle policy tests run without Android UI.
- Keyboard tests cover numpad Enter/separator, left/right modifiers, Print Screen and supplementary
  Unicode without truncation; dead keys and invalid code points are not sent as text. Virtual-key
  assignments follow [Microsoft's table](https://learn.microsoft.com/en-us/windows/win32/inputdev/virtual-key-codes).
  USB report tests cover disabled buttons, chained/many-to-one maps, face-swap precedence, release and
  preservation of paddles/touchpad/D-pad. Android and USB model IDs must match the saved profile.
- Audited every manifest Activity: PcView, AppView, AddComputerManually and StreamSettings already use
  Material 3 layouts; ControllerMappingActivity and HelpActivity now share that navigation shell.
  Game retains its streaming surface with a Material theme and explicit white telemetry text;
  ShortcutTrampoline has no content UI and uses the converted shared dialogs. The unused OSC container
  has no controls to convert. Remaining explicit dialogs (stream menu/touch/OSC/reconnect/commands/status/
  bitrate, profile, host details, bandwidth, errors/confirmations and loading) use Material builders and
  progress indicators. Native ListPreference/SeekBarPreference/language/reset-OSC dialogs retain the
  existing matching PlatformDialogTheme to preserve platform preference state/persistence.
- Role badges grow below artwork rather than occupying a fixed image area; TalkBack descriptions include
  role/action, and remote tiles no longer advertise global Quit. Host status text can grow. Profile
  controls and the OSC settings button have at least 48 dp targets. Controller rows are keyboard-focusable
  with grouped screen-reader content. Rendering verification below is still required.

### Current verification scope and real-device checklist

#### Adaptive windows and living-room controls (2026-10-09)

Rubylight uses Jetpack WindowManager (`window-java` and `window-core` 1.4.0) for live folding features and
Material window size classes (compact below 600 dp, medium below 840 dp, expanded above).
This dependency supplies hinge data unavailable from display metrics. Layout uses each window's
available content area, including system bars, caption bars, cutouts and the on-screen keyboard.
Separating book/tabletop folds and occluding hinges select the larger contiguous pane; ties use
the leading pane. Flat, non-separating folds use the whole window. This is a single-pane layout,
not a two-pane master/detail UI. Settings and manual host entry are bounded to 840 dp.

PC/app grids retain their views and focus during window resizing. Landscape app artwork is
shorter, TV tiles are larger, and mouse hover uses the existing Material 3 accent. The leanback
launcher retains its IDs and now displays Rubylight in the banner. TV settings start with Controls.
Controller mapping requires selecting **Map a button**, so ordinary gamepad presses can navigate.
Switch help is available with Right (Left in RTL) or F1; D-pad center toggles the selected row.
Library action sheets are available through Menu or Shift+F10 as well as long-press Select.

Outside streaming: Ctrl+N adds a PC, Ctrl+, opens settings, Ctrl+F focuses settings search,
F1 opens troubleshooting, and Esc/B navigates back. Streaming keeps its existing input shortcuts.
Large/freeform streaming windows allow either orientation. Split-screen stays at foreground
video quality and suppresses automatic PiP entry; returning fullscreen restores eligibility.
PiP source bounds follow the visible stream, and PiP drops browsing margins and hinge padding.

JVM/Robolectric coverage includes window breakpoints, hinge panes, unfold restoration, independent
window insets, keyboard and accessibility traversal order, preference help versus toggle, explicit
controller capture, library focus preservation and PiP eligibility. These checks do not certify
hardware rendering, OEM folding callbacks, remote key maps or screen-reader announcements.

Device checklist — all pending hardware verification:

- Tablet: PC and both app-grid sizes in portrait/landscape, 600/840 dp boundaries, font scales
  1.0/1.3/2.0, light/dark mode, long names, grid scrolling and preserved selection after rotation.
- Foldable: folded outer display, unfolded inner display, book and tabletop postures, flat display,
  hinge offset, RTL, and folding while settings, host entry, action sheets or streaming are open.
  Check that every control and the stream remain in the usable pane and that unfolding restores space.
- Android TV/Fire TV: launcher icon/banner; use only a remote/gamepad through discovery, pairing,
  PC/app actions, manual host entry, every settings group, search, sliders, switch help, host profiles,
  controller mapping, diagnostics, help and stream menus. Verify visible focus, Enter/A, long Select,
  Menu and Back/B, including cancellation and returning focus after dialogs.
- Chromebook/DeX/desktop: resize from narrow to wide and short windows; move between displays;
  check hover, wheel scrolling, Tab/Shift+Tab, all documented shortcuts and text-field editing.
  Switch focus between visible apps without unintended PiP or reduced stream cadence; enter PiP
  from fullscreen, restore, close and disconnect. Repeat with captured mouse input and an open menu.
- Insets/accessibility: gesture/three-button navigation, desktop caption bar, each cutout edge,
  keyboard show/hide, two windows with different insets, TalkBack traversal and focus returning from
  dialogs. Check Add PC address → Add and header → grid → actions without traps or obscured targets.

#### Low-latency policies and hostless diagnostics (2026-10-09)

Verification: JDK 17 `:app:assembleNonRootDebug` and `:app:testNonRootDebugUnitTest` pass with
`--no-daemon --max-workers=2`, a 1.5 GiB heap and two active processors: **643 tests**, including
28 new tests, zero failures/errors/skips. All four native ABIs compile. `:app:lintNonRootDebug`
still fails on **603 existing MissingTranslation errors in unchanged resource files** (213 warnings);
there are no new lint errors. The new benchmark provides English/German strings, with excluded-locale
translation checking suppressed only for that new resource file. No device, adb, emulator or live
host was used, so runtime/physical latency claims remain unverified.

Implemented and unit-testable without a device:
- Panel-rate input sampling reuses the existing immediate-input preference. Relative mouse motion
  sums without short overflow; absolute motion keeps the latest coordinate within the same reference
  size. Gamepad analog samples coalesce per slot, while digital/trigger-zero edges and detach flush
  immediately. The timer waits one panel interval after each flush, avoiding catch-up bursts after
  a scheduling stall. No API can raise a Bluetooth/USB controller's physical report rate.
- Balanced pacing uses a phase-preserving stream deadline (including 60 FPS on a 90 Hz panel) and
  the next display presentation deadline. Latency mode drains stale output; smoothness/cap modes do
  not drain it. With no new frame the compositor keeps the last image; no synthetic codec frame is
  enqueued. Confirmed Android ARR still releases immediately.
- Decoder limits apply only to client-held decoded output. Legacy OMX MTK/Exynos/Tegra/Broadcom/TI/
  Allwinner/Amlogic/Intel families retain one output; other families get a second balanced-pacing
  slot only with advertised low-latency support. Configuration retries and codec recovery fall back
  to one. Existing SPS/reference-picture fixups and vendor-key retries remain authoritative; newer
  Qualcomm, Tensor, MediaTek and Exynos names alone do not justify new codec tuning.
- AAudio is dynamically loaded, so API 21-26 still load the core library. API 27+ matching-rate stereo
  without effects tries exclusive then shared LOW_LATENCY callback output, checking the actual
  format/rate/mode. Failed setup and disconnection fall back to AudioTrack. Callback work uses a
  bounded preallocated ring; close/fallback happens on the serialized Java audio path. Measured
  native/producer underruns or AudioTrack underruns grow the buffer at most once per second;
  30 clean seconds permit a one-burst reduction. Capacity and the accepted initial minimum bound
  tuning; the normal upper bound is eight bursts. This is not a measured audio-latency claim.
- Both bitrate controllers consume ENet RTT variation as a control-channel jitter proxy, alongside
  residual frame/record loss and completed decode time (plus the existing PyroWave queue delay).
  Invalid/missing measurements prevent recovery increases. Cooldowns, host-applied caps and opt-in
  behavior remain. RTT variation is not a per-packet video-jitter measurement.
- The existing common-c Sunshine FEC-status feedback hook remains in use for compatible hosts
  (data/parity received, missing packets, block and FEC percentage; bounded best-effort feedback).
  The inspected host reference has no negotiated runtime FEC-percentage request endpoint. No
  speculative percentage request or change to PyroWave record negotiation is added.
- Settings → App → Latency benchmark runs without pairing/network access. Frame callbacks,
  their dispatch delay, 100 ms timer wakeups and Android input event age are measured for ten
  seconds, retaining up to 2048 recent samples per metric. Results distinguish reported audio/display
  properties from measured local timing. Input timestamps have millisecond resolution. Decode,
  scanout, audio-output and glass-to-glass values are unavailable; no synthetic value is substituted.

Android API basis: [MediaCodec presentation timestamps](https://developer.android.com/reference/android/media/MediaCodec#releaseOutputBuffer(int,%20long)),
[display deadlines](https://developer.android.com/reference/android/view/Display#getPresentationDeadlineNanos()),
and [AAudio callbacks, buffer sizing and disconnect handling](https://developer.android.com/ndk/guides/audio/aaudio/aaudio).

Still required on real devices:
- [ ] At 60/90/119.88/120/144/240 Hz, verify actual input send cadence, fractional mouse distance,
      rapid click/trigger pulses, all controller slots, detach/reconnect and the immediate-input
      override. Switch display modes and background/disconnect with pending motion.
- [ ] Compare all four pacing modes at matching and mixed stream/panel rates, including 60→90,
      60→120 and VRR/ARR fallback. Verify dropped vs held frames, callback-observed presentation,
      mode switches, GPU upscaling and decoder recovery; capture CSV and a high-speed-camera check.
- [ ] On the exact SoC/firmware/decoder, verify queue-starvation absence, first-frame output and
      static-to-motion/thermal behavior. Keep the existing 2024+ errata evidence limits.
- [ ] Confirm the actual AAudio sharing/performance mode, stereo/rate/effect/surround fallbacks,
      unplug/Bluetooth route switches, pause/stop/reconnect, underrun-driven growth and slow recovery.
      Measure output latency and A/V sync with physical equipment; inspect long-running audio.
- [ ] Inject sustained/bursty loss and jitter separately from decoder overload. Check both bitrate
      controllers' reduction/recovery, the 500 Mbps runtime cap and host FEC feedback handling.
      Dynamic FEC percentage control remains host/protocol work.
- [ ] Run the benchmark offline, with no input, with touch/mouse/controller input, and under load.
      Verify unavailable values, avg/p95/p99, ten-second completion, manual stop/restart, Back,
      rotation/background cancellation, large text, TalkBack and English/German layout.


First-device robustness pass (2026-10-09, `bp-jobD`): source review covered every production
Activity and Service listed in the manifest. No host code, dependency, commit or push was changed.

Final Windows/JDK 17 validation passed: `:app:assembleNonRootDebug`,
`:app:testNonRootDebugUnitTest`, `:app:lintNonRootDebug`, with `--no-daemon --max-workers=2`,
a 1.5 GiB heap and two active processors. **470 tests** passed (10 added), zero failures/errors/skips;
**lint: 0 errors, 180 warnings**. New tests cover incomplete launches, preset values and preference
writes, report redaction/bounds, connection measurement math and probe deadlines/cancellation.
The APK includes all four ABIs. No adb, emulator, real device, controller or host was used.

| Lifecycle owner | Review and changes |
| --- | --- |
| `Game` | Reject incomplete launch data before acquiring resources; restored Activities return to the library for an explicit resume. Guard late stream/PiP callbacks, release only held Wi-Fi locks, track pending USB bindings, clear delayed callbacks, stop on surface loss and finish after background connection failure. Existing native stop/start serialization and reconnect ordering remain in use. |
| `PcView`, `AppView`, `ShortcutTrampoline` | Unbind even before service connection; interrupt readiness waits; ignore expired UI work; keep worker-local binder references. Pairing cancels HTTP work when leaving and cannot navigate after cancellation. App-list polling start is idempotent. |
| `AddComputerManually` | Stop its worker without joining network I/O on the UI thread; retain its binder for in-flight work and reject results after interruption. |
| `HelpActivity` | Handle missing URLs and destroy its WebView and loading dialog. |
| `StreamSettings`, `ControllerMappingActivity` | Reviewed configuration restoration, preference listeners, button input and dialog ownership. Settings retain descriptions for resolution/FPS/bitrate and add preset, overlay and support actions; mapping uses the existing null-device checks. |
| `ComputerManagerService`, `DiscoveryService` | Cancel readiness waits at destruction; snapshot listeners before calling them; stop discovery and polling on teardown and unbind pending discovery connections. mDNS tolerates absent Wi-Fi/denied multicast locks; NSD ignores stale updates and lets idle callback threads exit. |
| `UsbDriverService`, `ControllerHandler` | Ignore incomplete/detached USB events, cancel delayed attachment work, pair permission completion with pending requests and stop on unbind. Snapshot listeners; serialize USB arrival/removal with the UI; concurrent controller maps tolerate detach during host feedback. Stop battery/sensor callbacks and prevent light sessions reopening after detach. |
| Shared dialogs | Dismissal is terminal even before a queued dialog is shown; cleanup is scoped to its Activity and tolerates missing views. |

The new connection-test screen reuses server-info requests and the host bandwidth probe. It shows
mean request latency, mean absolute variation between adjacent successful requests, and failed-request
percentage. These are HTTP measurements, **not UDP packet loss**. Missing samples remain unavailable;
unsupported bandwidth tests do not prevent latency testing. The test is cancellable on leaving.
The report shares at most 200 fixed-text events from the current process; raw diagnostics and all
host/user-supplied text are excluded. It does not preserve a prior process's crash log.

First-device checklist additions (all **not yet verified on hardware**):

- Cold launch with no hosts: use Add PC and Pairing help, then try guest Wi-Fi/VPN/network loss.
  Pair with ordinary PIN and one-time PIN; check incorrect/expired PIN, host busy, timeout,
  cancellation, Home/Back and retry. Rotate during PIN entry and pairing; cancelled work must not
  open an app list or another dialog later.
- Start and immediately stop a stream repeatedly; rotate and change window size while starting,
  streaming and disconnecting. Enter/exit PiP, disconnect through PiP and dismiss the PiP window.
  Check that audio, Wi-Fi locks, input capture, discovery and controller feedback stop afterward.
- Kill the client process during a stream using device system controls, then reopen it and resume
  explicitly. Disconnect Wi-Fi, reconnect and use Reconnect. Back must return to a usable library
  without a stale stream, spinner or duplicate session.
- Attach/detach USB DualSense during startup, gameplay, rumble and trigger effects; deny/cancel USB
  permission and unplug while permission is pending. Repeat Bluetooth connect/disconnect with gyro,
  lightbar and rumble active. Verify no stuck input or effects and no accumulating callbacks.
- Long-press a PC → Test your connection: test healthy, unreachable, unpaired and older hosts;
  interrupt Wi-Fi, cancel, leave, rotate and retry. Confirm unavailable values, HTTP loss labeling
  and bandwidth support handling. Follow Wake-on-LAN help and verify actual wake from sleep.
- Apply all four presets, reopen settings and start a stream. Check codec/bitrate/FPS/VRR/pacing,
  battery-performance switches and per-PC profile precedence. Review every help line at large font
  sizes on a narrow phone; settings values and descriptions must remain visible.
- Switch compact/expanded overlay, copy the stats line, paste it into a local text app and return;
  repeat with adaptive bitrate and PiP. Check text readability and controller/touch input afterward.
- After a failed pairing/stream, Settings → Report a problem: inspect the shared text file for the
  expected event order and confirm that no address (IPv4/IPv6/hostname/MAC), PIN, passphrase or raw
  response appears. Cancel sharing, return to the app and repeat with another share target.

Earlier usability-pass validation on Windows/JDK 17: `:app:assembleNonRootDebug`,
`:app:testNonRootDebugUnitTest` and `:app:lintNonRootDebug` passed with
`--no-daemon --max-workers=2`, a 1.5 GiB heap and two active processors. JUnit passed **460 tests**
(28 added), with zero failures, errors or skips; lint reported **0 errors and 180 warnings**.
There is no Robolectric, Paparazzi, Roborazzi or other JVM rendering/screenshot suite in this project;
the existing screenshot script requires an emulator and was not run. No adb, emulator, phone,
physical controller or host was used for this pass. No dependency, commit or push was added.

Earlier 2026-10-09 follow-up (before the usability pass), Windows/JDK 17: `:app:assembleNonRootDebug`,
`:app:testNonRootDebugUnitTest` and `:app:lintNonRootDebug` all passed with
`--no-daemon --max-workers=2`. The APK includes all four Android ABIs. JUnit passed
432 tests with zero failures, errors or skips; lint reported zero errors and 178 warnings.
The standalone native `pyrowave_frame_test` passed with GCC 16.2, including record loss,
format-change concealment, malformed/truncated input and negotiation cases. Dependencies
were copied to a writable worktree cache after the shared-cache ZIP cleanup failed.
No emulator, adb, phone or physical controller was used in this follow-up. Captured USB
input reports supplement synthetic Edge/effect fixtures; the hardware checks below remain open.

Historical milestone-7 verification (before the 2026-10-09 follow-up): the local checks covered
profile validation/round-trips, fractional/stock-host query behavior,
UUID/order/artwork identity, the trailing version sentinel, OTP hashing/request mode, bandwidth
streaming/cancellation boundaries and the automatic-bitrate state machine, alongside existing native
protocol/decoder tests. After rebasing onto the UI pass at `67e929b1`, `assembleNonRootDebug` passed for all four ordinary-codec ABIs,
`testNonRootDebugUnitTest` passed 86 tests with zero failures/errors/skips, and `lintNonRootDebug`
reported zero errors and 196 warnings (including two AV1 spelling warnings). The native
`pyrowave_frame_test` assertions passed with GCC 16.1. These runs used Windows/JDK 17 and Gradle
`--no-daemon --max-workers=2`; they do not certify a stream or real device.

In that earlier run, `scripts/emulator-smoke.py` passed on the headless Android 35 `sunset` AVD:
manual discovery, invalid profile input, saving/reopening 1920×1080 at 59.94 Hz, resetting to global
settings, overlay preference persistence, About's Moonlight/GPL-3.0 attribution, and production
overlay layout/rotation. Eleven screenshots were refreshed and the profile, settings, host list and
overlay captures were visually inspected. The crash buffer was empty; the script stopped its emulator
and `adb devices` was empty afterward. It waited until the previously running emulator had exited.
The fixture does not pair with a real host, decode a stream, measure a physical panel or certify
controllers. Earlier counts below belong to historical milestones.

On 2026-10-07 the same script also ran on CI (GitHub-hosted Ubuntu, KVM, Android 35 Google APIs
x86_64 AVD) through `workflow_dispatch` with a `prerelease_tag`. It now also injects a gamepad Y press,
maps it to A on **Controller buttons** and checks the stored mapping. That run builds a debug-signed
arm64-only APK (`-PabiFilters=arm64-v8a`) and publishes it with the screenshots as a GitHub pre-release.

The following still require real Android hardware and a host; passing synthetic tests is insufficient:

1. **Pairing/identity/permissions:** pair two installations with Rubylight rc.22 and stock
   Sunshine/Apollo; verify distinct host device/display identities, upgrades, restore-to-new-device,
   unpair/re-pair and certificate rejection. Exercise correct, wrong and expired OTP/passphrase,
   ordinary PIN pairing, permissions disabled/re-enabled/revoked mid-session, and View/Launch without
   List when opening a saved app shortcut. Check that pairing secrets are absent from logs/backups.
2. **Profiles/library:** save different dimensions/refresh/codec/HDR/range/chroma/scale on two hosts,
   reset one, and reconnect/launch through pinned shortcuts. Reorder/rename/remove apps, change IDs
   while retaining UUIDs, reuse an old numeric ID for another UUID, update artwork versions, and
   check running badges, cache refresh and Android TV poster/shortcut behavior.
3. **Display policy:** request portrait/native/custom sizes and 59.94/60/90/119.88/120/144/240 Hz where
   supported; inspect actual host output mode, client pacing, edge cropping and odd-dimension rounding.
   Test 100%/150% render scale, missing virtual driver, forced host/app/device overrides, layouts,
   display restoration and concurrent clients. Verify stock hosts keep compatible integer requests.
4. **Codecs/colour/PyroWave:** test H.264, HEVC, AV1 and explicit PyroWave, including unsupported codec,
   32-bit/API/Vulkan and TextureView fallbacks. Use HDR10 panels on Adreno and Mali/Immortalis for
   ten-bit ramps, fine 4:4:4 text, full/limited range, BT.2020/PQ, metadata changes/removal, HDR toggles,
   host ten-bit SDR, surface replacement and sustained thermal runs. Validate actual swapchain/decoder
   formats and completed-frame timing; no mobile PyroWave performance number is established here.
5. **Network adaptation/probe:** test wired and Wi-Fi routes with controlled loss/congestion/recovery,
   host caps, stale/no-video periods, rapid manual/automatic switches, permission revocation and
   disconnect during a request. Confirm bitrate reduces/recovers without oscillation or overlap and
   that stock hosts never acquire automatic control; PyroWave uses its separate opt-in controller. Run/cancel/background/timeout the
   bandwidth test, distinguish link speed from HTTP throughput, and compare with sustained UDP behavior.
6. **Input:** connect at least two physical USB/Bluetooth controllers, test slot order/hotplug,
   standard/vendor mappings, face swap, Settings → Controller buttons (remap, disable, reset, two
   different models, a mapped button held across the stream menu), sticks/deadzones, triggers, rumble, motion, battery and LEDs.
   Test OSC show/hide, move/resize/save, rotation and held-button release. Exercise native multi-touch,
   cancellation at Android edge gestures, unsupported-host fallback, trackpad/direct mouse gestures,
   stylus, USB/Bluetooth keyboards, non-QWERTY keys, AltGr/modifiers, IME Unicode, captured/absolute
   mice and focus/menu transitions without stuck keys/buttons. Root/evdev paths need separate hardware.
7. **Lifecycle/integration:** toggle overlay via menu and Ctrl+Alt+Shift+S; disconnect/resume/reconnect
   during startup, active video, HTTP actions and network loss. Verify the host app is preserved and
   no duplicate session, stale dialog, orphan decoder or accidental `/cancel` occurs. Exercise PiP
   entry/exit, Home/Back, rotation, screen lock, surface recreation, Android TV navigation, launcher
   shortcuts and Wake-on-LAN from actual sleep/offline states; check OEM background restrictions.
8. **Timing/audio/host actions:** compare processing avg/p95/p99 and CSV with host logs under sustained
   frame load, missing timing and loss; client render callbacks are not physical scanout. Test stereo,
   5.1/7.1 and route/encryption options, Unicode/empty/oversized clipboard with independent permissions,
   and server commands with reordered/empty/high indices, host app restrictions and disconnect.

9. **PyroWave records and rate control:** on the phone choose Video → Prefer PyroWave and start
   1280×720/60, then 1920×1080/120 if supported, first over USB Ethernet and then Wi-Fi. With a compatible
   Rubylight host confirm SDP `pyrowaveFeatures=1` and `186f0393`. On a controlled test link lose/reorder
   packets (including first, middle and last packets and a burst spanning a FEC block). Check surviving
   regions keep updating, affected regions recover with later records, and reconnect clears old imagery.
   Loss need not equal exactly one visible 64×64 block: one datagram can carry several records.
   Enable Automatic bitrate in the stream menu: sustained loss, queueing or decode time over the frame
   budget should lower bitrate; at least 20 healthy seconds with decode headroom should permit a slow rise.
   Repeat during a 20-minute thermal run, export the latency CSV and record overlay decode time/bitrate.
   Check the chosen/applied cap and 500 Mbps runtime cap, manual override, permission denial and reconnect.
   A host with unknown Rust version must use ordinary containers; an incompatible bitstream, unsupported
   Vulkan device or surface must fall back to AV1/HEVC/H.264. Compare ordinary-codec streaming afterwards.
10. **USB DualSense / Edge:** before enabling anything, connect by USB and verify ordinary Android
    controller input and no DualSense USB permission prompt. In Settings → Input turn on **USB DualSense
    driver**, read/cancel the Material 3 explanation, then enable it and start a stream on rc.23+.
    Deny the Android permission once: normal input should remain and the denial message should appear.
    Unplug/replug, allow permission, and verify one player slot, sticks, hat/buttons, analog triggers,
    both touch contacts, gyro/accelerometer, battery, player LEDs, lightbar RGB and each rumble motor.
    Exercise host-driven left-only, right-only, simultaneous and Off trigger effects (feedback, weapon,
    vibration and a title using zone effects); interleave lightbar/rumble changes and verify the other
    trigger is preserved. On Edge press each back paddle separately/together and each Fn button; only
    the paddles should send paddle flags (rc.24+ mapping). Repeat with a second pad, cable removal while
    effects are active, reconnect, stream exit and the toggle disabled. Rumble/triggers must reset on
    exit; no duplicate slot, stuck button, permission loop or blocked reconnect. Confirm the USB audio
    interfaces stay available; audio haptics are not implemented. Bluetooth keeps Android's input path.
    Record pad PID/firmware and phone OS, and retain USB input/output captures for Edge and actual host
    effects: those fixtures are still synthetic here. Physical input fixtures in JUnit come from the
    [published DS5/PS5 USB sniffer capture](https://github.com/matlo/GIMX/issues/672#issuecomment-1019605142),
    not this phone; they do not certify Android transfers or feedback strength.
11. **Role lifecycle and confirmation:** pair a second client and keep a game active there. Start
    Remote Monitor: video/audio should work and keyboard/touch/gamepad input must not reach the host.
    Start Input-only with the host option enabled: input/feedback should work without video/audio or
    a missing-video timeout (leave it connected at least a minute). Disconnect locally, reopen/rejoin,
    background/foreground, lose the network and reconnect. The main game must survive. Exercise the
    role-specific End remote session action: only that role should end. Refresh the app list and try
    Resume after ownership changes. For a replacement app and Terminate, test Cancel, Back, Home,
    rotation, a wait beyond 60 seconds and Confirm. Only an explicit timely Confirm may send the second
    request; successful Terminate/disconnect should close the tile without a stream failure. Verify
    unknown 410/conflict/permission-denied replies display an error and never repeat a destructive action.
12. **2024+ decoder evidence:** record the exact phone/SoC, OS/build, decoder name, advertised low-latency
    support, codec/profile, resolution/rate and pacing mode with each CSV. Compare H.264/HEVC/AV1 at
    1080p60/120 and native resolution, static scenes followed by motion, display power-save/full-resolution
    modes and at least 20 minutes of heat/load. For a 9300-class device include >1440p static-to-motion;
    for S24+ Exynos include the upstream workload-dependent stutter scenario; for 8 Gen 3 include sustained
    AV1/HDR/120. Keep network loss separate from decode/render stalls. Elite/Tensor G4 timing and useful
    vendor keys remain unknown until measured; see [decoder-errata.txt](../decoder-errata.txt). No new
    SoC tuning was added on the strength of issue reports, and no phone timings were generated locally.

13. **ARR / fixed-rate fallback:** compare Android 15 SurfaceView and TextureView, Android 16 with
    and without ARR, and an older fixed-rate device. Enable VRR and vary stream cadence through
    30/59.94/60/90/119.88/120, a static scene, idle/no frames, resume and surface recreation. Compare
    **ARR**, **cadence hints**, or **max Hz** in the overlay with actual display diagnostics. Check
    hint updates do not cause black-screen mode switches on Android 15/16, touch boosts do not dominate
    supported ARR, and disabling VRR restores ordinary pacing. Battery Saver, thermal policy, HDR and
    OEM mode restrictions can override hints.
14. **PiP / USB mapping / wake:** try 16:9, portrait and 32:9 PiP. Use system Disconnect, expand,
    close, re-enter and reconnect; the host app must remain running and stale controls must do nothing.
    Input-only must not enter PiP. Save USB model remaps before a stream, including A→B, B→X, two
    buttons→A and a disabled button; test simultaneous hold/release, face swap, focus loss, disconnect
    and reconnect. For wake, test Wi-Fi plus Ethernet/VPN, different subnet masks and custom ports,
    one broken route, stale ARP, DNS failure, sleeping NICs and router/firewall policy.

**Caller emulator checklist (not run here):** use both **360 × 800 dp** and **393 × 851 dp**, portrait
and landscape, with font scales **1.0, 1.3 and 2.0**, plus gesture and three-button navigation.
Check light/dark system settings, long host/app names and at least one long-translation locale.

- Visit host list, app library (both tile sizes, running/hidden/monitor/input/end-role tiles), Add PC,
  all five settings sections, search/empty results, Controller buttons and Help. Confirm readable wrapping,
  scrolling, visible navigation, no clipped buttons, and bottom rows clear of IME/system insets.
- Search title/description/category words, accented text and punctuation. Select a result, change a
  value, go Back, clear the query and rotate with a query active. Unsupported preferences must stay absent;
  dependencies must still work after search (OSC/vibration, virtual display/render scale, HDR).
- Open per-host profile (inputs, validation errors, Match screen, Save/Reset), OTP, pairing/error/loading,
  quit/delete/reset, USB explanation, About, host details, bandwidth and every stream subdialog (touch mode,
  controller layout, bitrate, commands, status, confirmation/reconnect). Scroll to all content at 200% font;
  dialog buttons and slider +/-/presets must remain reachable. Use the caller's host/fixtures for stream UI.
- With TalkBack, traverse icon buttons, host/app cards and role labels, form fields/errors, preferences,
  search, controller rows and dialog buttons. Check each action is named once, headings are navigable,
  focus returns after dismissing dialogs, and touch bounds are at least 48 dp. Test keyboard/D-pad too.
- Stream in trackpad, direct touch and native multi-touch modes; open/cancel the Material touch selector
  during taps/drag/scroll, use three fingers, focus loss, Android edge cancellation and mouse capture.
  No stuck key/button or delayed release should affect the next gesture. These are device checks,
  not certifications from the JVM policy tests.

## Milestone 5: extras and packaging (historical record)

The latest host was cloned with `git clone --depth 1` outside this checkout and was only read.
Sources: [HTTP/permissions/clipboard/bitrate](https://github.com/RamazanKara/Rubylight/blob/f06e72c73c1960665d031d27f8dad02202bca56e/rust/host/src/nvhttp.rs),
[encrypted server commands](https://github.com/RamazanKara/Rubylight/blob/f06e72c73c1960665d031d27f8dad02202bca56e/rust/host/src/stream.rs),
[pause status](https://github.com/RamazanKara/Rubylight/blob/f06e72c73c1960665d031d27f8dad02202bca56e/rust/host/src/web.rs),
and [limiter policy](https://github.com/RamazanKara/Rubylight/blob/f06e72c73c1960665d031d27f8dad02202bca56e/docs/configuration.md).
Artemis Android was inspected at
[`c5cf27f4`](https://github.com/ClassicOldSong/moonlight-android/tree/c5cf27f4dc822db0e863c4691e7a70c74bea977a),
with common-c [`c9994368`](https://github.com/ClassicOldSong/moonlight-common-c/blob/c999436858471dfefa7617af3b7dc03ec1644ce4/src/ControlStream.c).
Its GPL-3.0 license permits reuse. This implementation follows its protocol approach, retains
Moonlight attribution, and leaves the pinned common-c submodule unchanged.

Rubylight requires exactly one command-index byte; Apollo uses that byte plus three zeros.
A local native translation unit shares the core's encryption, sequence numbering, ENet mutex and
reliable delivery. Empty command names preserve their index. Commands are refreshed before sending;
changed names/indices require reopening the menu. The UI reports sending, not successful execution.

Clipboard transfer is manual and text-only; no polling or automatic export. Incoming text is only
installed while foreground and focused, and marked sensitive on supported Android versions.
Non-text, oversized, HTTP-error and XML error-page replies cannot replace the clipboard. Android/OEM
clipboard services may have a smaller limit than the host's 1 MiB. Pause disconnects the stream,
leaving the application running; there is no client `/pause` endpoint or process-suspension claim.
Runtime bitrate is exposed only for `RustHostVersion` hosts, without probing stock-host endpoints.

Settings now group Display, Latency, and Codec and color, preserving keys, values and hardware gates.
Defaults remain 720p/60, auto codec, minimum-latency pacing, supported decoder low-latency hints,
SurfaceView, and opt-in HDR/4:4:4/virtual displays. Rubylight has a vector/adaptive icon and the
existing separate `com.butterpollo.client` package. README covers debug install and local release signing.
The two Ubuntu/JDK 17 workflows use two Gradle workers/native jobs and 15-minute timeouts: debug +
tests on `main`/manual dispatch; unsigned release + tests and release attachment on `v*` tags.
No signing secrets are used. GitHub-hosted execution needs its first published run; nothing was pushed.

Local verification passed on Windows/JDK 17 with Gradle `--no-daemon --max-workers=2`, a 1.5 GiB
heap and two active processors: `assembleNonRootDebug`, `testNonRootDebugUnitTest` (42 tests,
zero failures/errors/skips), `assembleNonRootRelease`, and `lintNonRootDebug` (zero errors,
201 warnings). Both APKs contain ARM64, ARMv7, x86 and x86-64 native command bindings.
The debug signature verifies; release signature verification fails as expected for an unsigned
APK, and its 16 KiB zip alignment passes. Workflow YAML/triggers and preservation of every
preference key/value/default were checked. The root flavor and real-device behavior were not tested.

Milestone 5 real-device checks, in addition to the earlier streaming checklist below:

- Install debug and locally signed release APKs on Android 5+, current Android and Android TV.
  Inspect legacy/adaptive/themed icons, pairing/discovery text, settings and HDR filtering; verify
  same-key updates retain settings/pairing and Rubylight coexists with Moonlight.
- Pair with Rubylight, Sunshine and Apollo. Exercise independent list/view/launch/input masks,
  zero permissions, revocation and re-enabling; check input-disabled notices and quit restrictions.
  Check ordinary and synthetic resume tiles. Full remote-monitor/input-only lifecycle UX remains deferred.
- Open, navigate and dismiss the menu with touch Back, TV remote and keyboard shortcut. Verify
  capture release/restoration, controller input, PiP suppression, backgrounding, rotation, and
  disconnect during an HTTP action without late dialogs or clipboard writes.
- Transfer Unicode, multiline and empty text in both directions; test independent permissions,
  non-text clips, size/OEM limits, clipboard contention, denied/expired sessions, sensitive previews,
  and absence of clipboard contents in logcat.
- Run commands at indices 0, 1 and 255 on Rubylight and Apollo, including unnamed entries,
  reordered lists, permission/app-level denial, rapid repeats, packet loss and disconnect. Verify
  exactly the selected command runs; there is no host execution acknowledgment.
- Change runtime bitrate and verify encoder rate/host caps, failure after session termination,
  and restored startup settings on reconnect. Compare displayed limiter state and fractional caps
  with the host console; Android pacing must remain unchanged and no VRR request should be sent.
- Disconnect and resume the same running game, including background/foreground and rotation.
  Verify the game stays running and no unintended `/cancel` occurs.

## Milestone 4b: Vulkan decoder port (historical implementation record)

The implementation notes and measurements in this section describe that milestone. The current
feature status, newer verification and remaining work are recorded in milestone 7 above.

The second attempt ports the Android renderer from
[joemossjr16/artemis-android-pyrowave `387d3a5c`](https://github.com/joemossjr16/artemis-android-pyrowave/tree/387d3a5ce1e3df8d4a4d29eec5b81a7b904926a5).
Its [LICENSE.txt](https://github.com/joemossjr16/artemis-android-pyrowave/blob/387d3a5ce1e3df8d4a4d29eec5b81a7b904926a5/LICENSE.txt)
is GPL-3.0, compatible with this GPL-3.0 Moonlight-derived client. Attribution remains in source,
the application license, and packaged `assets/pyrowave_notices.txt`. PyroWave, Granite and volk's
full MIT notices are included there. The necessary new runtime dependency is `libpyrowave-shared.so`.
Both ARM64 and x86-64 libraries were rebuilt with Windows NDK 29, rather than copied from the fork.
32-bit APKs retain the ordinary decoders and tolerate the absent Vulkan library.

Pinned dependency sources: PyroWave `a0d2e71e98b85f6a23f62407df21a39e9745b875`, Granite
`fb178c8080d163419e8d20f10715c61c53c1ec9b`, volk `47cddf7ed97b94118a08aacb548a411188e016cc`,
Vulkan-Headers `6802bb4733b63ed5efd3adb308a6c885ef180ea1`. The decoder short-block patch from
Rubylight `6772d401` is retained unchanged in `jni/pyrowave-renderer/patches/`. Two additional
patches correct metadata read synchronization and the initial transition of aliased linear
payload images; see the rebuild notes below. Sources/headers are MIT; the small Android
renderer and common-c integration remain GPL-3.0.

### Negotiation and output

- **Prefer PyroWave** explicitly opts into the experimental decoder and uses **SurfaceView**.
  Existing HDR and 4:4:4 preferences control its profile bits; Auto keeps conventional codecs.
  The quality pass changed this from automatic selection because normal Moonlight bitrates are
  too low for PyroWave. Ordinary codec fallback remains available; start near 280 Mbps for 720p/60
  on a fast LAN and verify quality and bandwidth on the actual host/device.
- Android API 29+, a 64-bit library and Vulkan **1.3** are necessary. Runtime checks include
  subgroup basic/vote/ballot/arithmetic/shuffle/shuffle-relative operations, compute stage and size
  control (4–128), full subgroups, shaderInt16, storageBuffer8BitAccess, storageBuffer16BitAccess,
  timelineSemaphore, synchronization2 and shaderStorageImageWriteWithoutFormat. Float16 arithmetic
  is optional. Checks also cover graphics+compute+present queue support, scratch image formats,
  dimensions, layers and workgroup limits. These are conservative requirements of this port.
- After server capability intersection, the client creates the decoder, swapchain and colour pipeline
  on the **actual stream Surface**, then executes a zero-coefficient grey frame into the decode planes.
  This warm-up is not presented or included in stream statistics. A failed profile tries the next
  compatible profile; failure of all PyroWave profiles leaves conventional codec negotiation intact.
  The prepared renderer is reused by video setup and released when RTSP selects a conventional codec.
- Native RTSP requires `PYROWAVE/90000` and the exact `x-ss-pyrowave.bitstream:186f0393` attribute.
  Missing/different IDs select AV1/HEVC/H.264. SDP sends `bitStreamFormat=3`, the selected chroma and
  HDR flags. It deliberately omits **both** `pyrowaveAdaptiveFec` and `pyrowaveFeatures`; even a zero
  adaptive-FEC attribute selects a different framing protocol on Rubylight.
- The ordinary core FEC/depacketizer supplies complete opaque frames with its eight-byte short
  header and trailing FEC padding removed. The renderer accepts little-endian packet count/lengths,
  validates exact boundaries and block sizes, then feeds the packets to the pinned decoder. It does
  not accept the fork's older `PYRW` container or partially recovered record-mode frames. Corrupt
  frames are discarded, and the next complete intra frame recovers. The core submodule remains pinned;
  `android_rtsp.c` and `android_sdp.c` are attributed copies of its two modified translation units.
- Eight-bit output uses R8_UNORM planes; ten-bit output uses **R16_UNORM** throughout the decoded
  planes, including full-size chroma for 4:4:4. The shader matches Rubylight's code/1023 encoding,
  64–940 luma, midpoint 512/scale 896 chroma, optional full range, centred 4:2:0, and BT.709 SDR or
  BT.2020 non-constant-luminance HDR. HDR RGB remains PQ encoded, with no sRGB framebuffer conversion.
- HDR requires an A2B10G10R10 or A2R10G10B10 UNORM + `HDR10_ST2084_EXT` surface pair,
  `VK_EXT_swapchain_colorspace`, and `VK_EXT_hdr_metadata`. The same ten-bit format must also support
  SDR presentation, preserving ten-bit SDR when the host turns HDR off. Mode changes recreate the
  swapchain. Mastering primaries/white point, luminance and MaxCLL/MaxFALL are passed with Vulkan units;
  absent/short metadata clears old values to unknown. The unsupported trailing full-frame field is
  omitted. [Vulkan metadata units](https://docs.vulkan.org/refpages/latest/refpages/source/VkHdrMetadataEXT.html)
  and [colour-space definitions](https://docs.vulkan.org/refpages/latest/refpages/source/VkColorSpaceKHR.html)
  govern this path; MediaCodec HDR keys are not used for the Vulkan surface.
- Decode has its own fence, including when swapchain acquisition times out or requires recreation.
  PyroWave disables direct submission so GPU/display waits run on the core's decoder thread.
  Its CLOCK_MONOTONIC completion feeds milestone 1 `FrameLatencyStats` and the existing CSV/overlay.
  Input-to-output includes parsing/upload/queue wait and GPU completion, but excludes subsequent
  image acquisition/present. Separate GPU timestamps bracket the GPU decode. Render/scanout time
  stays unavailable; no fabricated render callback or rendered-FPS count. Fatal runtime decoder
  errors end the connection; they do not attempt an in-place codec switch. Reconnect with explicit
  HEVC/AV1 for a driver failure occurring after successful preparation.

### Measured synthetic frames

On 2026-10-07, `app/src/test/native/pyrowave_benchmark.cpp` used the pinned upstream Windows encoder
to generate 1280×720 luma ramps/chroma checkerboards, packetized at 1024 bytes into the same container
the Android renderer accepts. The two frames were 501,996 (4:2:0) and 502,656 (4:4:4) bytes, with a
500,000-byte codec budget. Each of four runs discarded 10 warm-up decodes and measured 60 completed
decodes; submissions were spaced by at least 17 ms and the process was restricted to two CPUs.

The GPU selected by the upstream C API was **AMD Radeon RX 7900 XT**, Vulkan 1.4.349, raw driver
version 8389003. This was the existing Windows toolchain build of the same pinned C API; DLL SHA-256
`090423bf0054353f29b3cf6ac7cfab76a59cdc7bec369ae8c0564fdb825f8a31`.
`PyroWaveBenchmark.java` feeds the native monotonic input/completed-output timestamps into the actual
milestone 1 statistics class. The recorded input is checked in at
`app/src/test/resources/pyrowave/windows-720p.csv`; the adapter can regenerate all four milestone 1 CSVs.

| 720p, 8-bit | Completed frames | Input → output avg | p95 | p99 |
| --- | ---: | ---: | ---: | ---: |
| 4:2:0 compute | 60 | 15.850740 ms | 44.936700 ms | 110.844200 ms |
| 4:2:0 fragment | 60 | 34.978430 ms | 135.435400 ms | 203.041700 ms |
| 4:4:4 compute | 60 | 26.796847 ms | 93.904900 ms | 126.660700 ms |
| 4:4:4 fragment | 60 | 24.810995 ms | 75.169600 ms | 108.253700 ms |

These are **Windows synchronous GPU decode + CPU readback** timings on a shared, CPU-constrained
machine, with large scheduling/contention tails. They are neither GPU-only nor Android renderer
measurements, and establish no mobile performance target. Receive/render timestamps are left blank.
The zero-coefficient warm-up was separately checked through the upstream compute and fragment paths
for both chroma modes, producing grey pixels. No Android device/emulator was connected; Android
surface output, ten-bit visual accuracy, HDR transitions, live transport and mobile timing remain
unverified. The shaders compile and pass SPIR-V validation; that does not certify display output.

To repeat the Windows benchmark from this repository in PowerShell (MSYS2 UCRT64 compiler, Windows
executables; use a matching C API installation in `$PyroInstall`):

```powershell
$Bench = Join-Path $env:TEMP 'bp-pyrowave-bench'
New-Item -ItemType Directory -Force $Bench | Out-Null
$PyroInstall = 'C:/Users/ramaz/git/pyrowave-build/install-502a3b52'
$VulkanHeaders = 'C:/Users/ramaz/git/pyrowave-build/src-502a3b52/Granite/third_party/khronos/vulkan-headers/include'
$env:PATH = 'C:/msys64/ucrt64/bin;' + $env:PATH
g++ -std=c++17 -O2 app/src/test/native/pyrowave_frame_test.cpp -o "$Bench/frame-test.exe"
& "$Bench/frame-test.exe"
g++ -std=c++17 -O2 -Iapp/src/main/jni/pyrowave-renderer/prebuilt/include "-I$VulkanHeaders" app/src/test/native/pyrowave_benchmark.cpp "$PyroInstall/lib/libpyrowave-shared.dll.a" -o "$Bench/benchmark.exe"
Copy-Item "$PyroInstall/bin/libpyrowave-shared-1.dll" $Bench
$p = Start-Process "$Bench/benchmark.exe" -WorkingDirectory $Bench -WindowStyle Hidden -PassThru -RedirectStandardOutput "$Bench/measurements.csv" -RedirectStandardError "$Bench/benchmark.log"
$p.ProcessorAffinity = 3
$p.WaitForExit()
if ($p.ExitCode -ne 0) { throw 'Benchmark failed; inspect benchmark.log' }
javac -J-XX:ActiveProcessorCount=2 -d "$Bench/classes" app/src/main/java/com/limelight/binding/video/FrameLatencyStats.java app/src/test/java/com/limelight/binding/video/PyroWaveBenchmark.java
java -XX:ActiveProcessorCount=2 -cp "$Bench/classes" com.limelight.binding.video.PyroWaveBenchmark "$Bench/measurements.csv" $Bench
```

### Rebuilding the bundled decoder

Run from the repository root. Use a fresh task directory on D: when available; normal Gradle
builds consume the committed prebuilts, so no downloads or CMake build run during ordinary
application builds. No owner checkout, system services or drivers are modified by these commands.

Apply all three patches in filename order. `0003` retains the malformed short-block rejection.
`0004` includes storage-buffer reads in the post-upload dependency: dequantization metadata
is always an SSBO, even when payload data uses sampled texel buffers or linear images.
`0005` suppresses Granite's automatic initial image transition. LinearHost still creates the
Vulkan image as PREINITIALIZED, and the existing first upload performs its sole transition
to GENERAL. Subsequent uploads retain GENERAL. Neither patch changes the public API,
bitstream, precision settings or embedded SDK shaders.

```powershell
$Repo = (Get-Location).Path
$Work = if (Test-Path 'D:/') { 'D:/CodexArtifacts/nightjar-pyrowave-rebuild' } else { Join-Path $env:TEMP 'nightjar-pyrowave-rebuild' }
if (Test-Path -LiteralPath $Work) { throw 'Use a fresh rebuild directory.' }
$Source = "$Work/pyrowave"
$Sdk = 'C:/Users/ramaz/AppData/Local/Android/Sdk'
$Ndk = "$Sdk/ndk/29.0.14206865"
$env:PATH = "$Sdk/cmake/3.22.1/bin;" + $env:PATH
git init $Source
git -C $Source remote add origin https://github.com/Themaister/pyrowave.git
git -C $Source fetch --depth 1 origin a0d2e71e98b85f6a23f62407df21a39e9745b875
git -C $Source checkout --detach FETCH_HEAD
git init "$Source/Granite"
git -C "$Source/Granite" remote add origin https://github.com/Themaister/Granite.git
git -C "$Source/Granite" fetch --depth 1 origin fb178c8080d163419e8d20f10715c61c53c1ec9b
git -C "$Source/Granite" checkout --detach FETCH_HEAD
git -C "$Source/Granite" submodule update --init --jobs 2 third_party/volk third_party/khronos/vulkan-headers
Get-ChildItem "$Repo/app/src/main/jni/pyrowave-renderer/patches/*.patch" | Sort-Object Name | ForEach-Object {
    git -C $Source apply $_.FullName
    if ($LASTEXITCODE -ne 0) { throw "SDK patch failed: $($_.Name)" }
}
foreach ($abi in @('arm64-v8a', 'x86_64')) {
    cmake -S $Source -B "$Work/$abi" -G Ninja "-DCMAKE_TOOLCHAIN_FILE=$Ndk/build/cmake/android.toolchain.cmake" "-DANDROID_ABI=$abi" -DANDROID_PLATFORM=android-29 -DANDROID_STL=c++_static -DCMAKE_BUILD_TYPE=Release -DANDROID_SUPPORT_FLEXIBLE_PAGE_SIZES=ON -DPYROWAVE_FP32_MATH=ON -DPYROWAVE_FP32_STORAGE=OFF -DPYROWAVE_DEVEL=OFF -DPYROWAVE_UTILS=OFF
    if ($LASTEXITCODE -ne 0) { throw "SDK configure failed: $abi" }
    cmake --build "$Work/$abi" --target pyrowave-shared --parallel 2
    if ($LASTEXITCODE -ne 0) { throw "SDK build failed: $abi" }
    Copy-Item "$Work/$abi/libpyrowave-shared.so" "app/src/main/jni/pyrowave-renderer/prebuilt/$abi/"
    & "$Ndk/toolchains/llvm/prebuilt/windows-x86_64/bin/llvm-strip.exe" --strip-unneeded "app/src/main/jni/pyrowave-renderer/prebuilt/$abi/libpyrowave-shared.so"
}
```

The 2026-10-17 combined rebuild used Windows NDK `29.0.14206865` (Clang 21, build
`13989888` / `r563880c`), SDK CMake `3.22.1-g37088a8-dirty` and its bundled Ninja, with the
pins and explicit options above. Both libraries retain all 48 exported API names, the SONAME,
Android system-library dependencies and 16 KiB ELF LOAD alignment. The stripped artifacts are:

| ABI | Bytes | SHA-256 |
| --- | ---: | --- |
| arm64-v8a | 1,233,704 | `6f7626d783ad1aeaa0181f2cb3a0608cd3887b821fa9807cd34e032f76e7ea4e` |
| x86_64 | 1,284,248 | `8a5bbda825985392eb5acc56b2fe7ec955b0433b882f5d508a0f4e3f3da69adc` |

On 2026-10-20 the libraries were rebuilt with the same toolchain, options and three patches at
PyroWave `a0d2e71e`, which changes only the fragment iDWT shader (`shaders/idwt.frag` and its
embedded SPIR-V): the first two outputs at an edge now apply the edge mirroring, and the CbCr
mirroring for multi-plane input is no longer compiled out by a misnamed macro. The fragment path is the one
upstream selects on Qualcomm drivers. Granite, the C API, the bitstream and the patches are
unchanged; all 48 exports, the SONAME and 16 KiB LOAD alignment were rechecked.

| ABI | Bytes | SHA-256 |
| --- | ---: | --- |
| arm64-v8a | 1,234,792 | `4239343a6a15194c2c82ce881767f48fd6beb9e9901bbad6e1ac4c76fd4cf48b` |
| x86_64 | 1,285,336 | `50ef087792580309ce5e3f135e25090d47792e78fffcde0c7c4bfbffab165283` |

These are artifact identities, not a claim of path-independent reproducible binaries. The
ARM64 device comparison on Fold7 with active Turnip Gen8 V37 tested a rebuilt control
(short-block patch only), metadata barrier only, and both synchronization patches. All three
still showed the chart corruption with 0% reported missing records. These fixes correct
conditional synchronization defects; they do not resolve that reported visual corruption.
The combined rebuild rendered the same HDR chart correctly with the system driver and
also completed the native-resolution moving-scene checks on that driver.

To regenerate `shaders_spv.h`, compile each `shaders/fullscreen.vert`, `shaders/planar_csc.frag` and
`shaders/sgsr.frag` with NDK `shader-tools/windows-x86_64/glslc.exe -O --target-env=vulkan1.3 -mfmt=c`, and put
those initializer lists in `static const uint32_t fullscreen_vert_spv[]` / `planar_csc_frag_spv[]` / `sgsr_frag_spv[]`.
Also compile binary SPIR-V and run `spirv-val.exe --target-env vulkan1.3` on it.

### Exact real-device checks still required

Local verification passed with Windows JDK 17, the specified SDK, Gradle `--no-daemon --max-workers=2`,
two active processors and a 1.5 GiB heap: `assembleNonRootDebug`, `assembleNonRootRelease`,
`testNonRootDebugUnitTest` (46 tests, zero failures/errors/skips), and `lintNonRootDebug` (zero errors,
201 existing warnings). Both APKs contain all four ordinary-codec ABIs and the two 64-bit PyroWave
ABIs, with the complete notices preserved in release assets. Release 16 KiB zip alignment passed.
The native parser/RTSP assertions and the 240-frame milestone 1 replay passed; the standalone ARM64
benchmark also compiled against the bundled decoder. The root flavor and real-device checks below
were not run.

1. Build with JDK 17, the specified SDK, `gradlew.bat --no-daemon --max-workers=2
   assembleNonRootDebug testNonRootDebugUnitTest`. Native make is already capped at `-j2`.
   Find the APK under `app/build/intermediates/apk/nonRoot/debug/` with AGP 9.4, then
   `adb -s SERIAL install -r app/build/intermediates/apk/nonRoot/debug/app-nonRoot-debug.apk`.
2. On a Vulkan 1.3 Adreno and a Mali/Immortalis device, set Codec **Prefer PyroWave**,
   enable the performance overlay, and start at 1280×720/60. Pair with Rubylight built with its
   `186f0393` PyroWave encoder. Use adequate LAN bitrate (e.g. 200,000 kbps on gigabit Ethernet).
   Capture `adb -s SERIAL logcat -v threadtime` to a file during the run. Verify the PyroWave surface
   preparation and selected decode path logs, actual codec/chroma in the overlay, and host SDP
   `bitStreamFormat=3`. Check that neither record-mode attribute is sent.
3. Repeat with 4:4:4 on/off, HDR on/off, full range on/off, and host `prefer_sdr_10bit`. On an HDR10
   panel, display ten-bit grey ramps, black/white patches, PQ highlights and single-pixel red/blue
   text on the host. Confirm the actual SurfaceView's 10-bit ST2084 swapchain in Vulkan validation /
   SurfaceFlinger diagnostics, BT.2020 primaries, no eight-bit intermediate/banding, correct blacks,
   centre chroma and full-resolution colour text. Compare against a known-good host/local image.
   Change/remove mastering/MaxCLL/MaxFALL metadata; toggle host HDR while connected. Confirm no stale
   metadata, a new SDR colour-space swapchain on HDR-off and preserved ten-bit planes.
4. Pause/disconnect/resume, background/foreground, rotate, replace/destroy the surface, and stream
   while the host changes HDR. Verify objects and decode/presentation fences are released, no hangs
   or repeated invalid semaphore use, and no validation-layer errors. Apply temporary packet loss/
   reordering on a test network: complete recovered frames decode; unrecoverable frames drop and
   the next intra frame recovers. Feed truncated/bad packet lengths and duplicate short blocks through
   the native parser regression test; no loops or out-of-bounds reads are permitted.
5. Stop normally to finalize the milestone 1 CSV, then extract it using
   `adb -s SERIAL exec-out run-as com.butterpollo.client cat files/butterpollo-latency.csv > device.csv`.
   During streaming use `files/butterpollo-latency.csv.tmp`. Report input-to-output avg/p95/p99 and
   sample count, plus GPU-decode log/overlay timing separately. Render fields must remain blank with
   `render_unavailable`. Repeat 1080p/native resolution at 60/120/240 Hz where supported, including
   a thermal run; never substitute Windows timings for the device's measurements.
6. Repeat with a 32-bit/API <29/Vulkan <1.3 device, TextureView, explicit HEVC/AV1, a missing-feature
   Vulkan driver, and an HDR surface without the required format/extension pairs. Verify conventional
   fallback, including HDR preference over PyroWave SDR. On a test host remove/change the SDP bitstream
   ID and verify AV1/HEVC fallback; repeat with ordinary Sunshine/Apollo and H.264-only hosts.

For an on-device synthetic C API benchmark independent of a host, compile
`app/src/test/native/pyrowave_benchmark.cpp` with NDK's `aarch64-linux-android29-clang++.cmd`,
`-std=c++17 -O2 -static-libstdc++`, include `jni/pyrowave-renderer/prebuilt/include`, and link with
`-Lapp/src/main/jni/pyrowave-renderer/prebuilt/arm64-v8a -lpyrowave-shared`.
Push the executable and the matching `.so` into `/data/local/tmp/bp-pyrowave/`; run
`adb -s SERIAL shell 'cd /data/local/tmp/bp-pyrowave && chmod 700 benchmark && LD_LIBRARY_PATH=. ./benchmark > measurements.csv'`.
Pull `measurements.csv` and feed it to the Java adapter above, then remove that temporary directory.
This measures complete GPU decode plus readback; the live-stream CSV is the Android Surface renderer
measurement. No instrumentation/emulator or mobile performance result is claimed from this run.

## Milestone 4: first-attempt audit (superseded by 4b)

The following records the first attempt at `c23f1357`; its disabled status and zero-frame
measurement apply to that historical attempt, not the implementation above.


**Mobile decoding is technically plausible and already demonstrated for SDR. This audit does not
establish that mobile GPUs cannot decode PyroWave.** The requested Android Vulkan decoder, full
ten-bit HDR 4:4:4 presentation and per-frame decode benchmark remain unimplemented. The safe result
of this run is an explicit negotiation exclusion, fallback regression tests and the evidence below,
not a completed decoder or an assertion that the task's mobile-infeasibility condition was met.

On 2026-10-07, cloned the latest
[Rubylight `6772d4019c0837e372b801637f3bf1b57bea2765`](https://github.com/RamazanKara/Rubylight/tree/6772d4019c0837e372b801637f3bf1b57bea2765)
with `git clone --depth 1` into a temporary directory outside this repository; it was only read.
Also inspected current upstream
[PyroWave `c0b997f84ced7bd827ca737aa5145f4ec811de8d`](https://github.com/Themaister/pyrowave/tree/c0b997f84ced7bd827ca737aa5145f4ec811de8d)
and the exact host compatibility target,
[PyroWave `502a3b52a39312ab82c85b1e2fc0e746faee91a4`](https://github.com/Themaister/pyrowave/tree/502a3b52a39312ab82c85b1e2fc0e746faee91a4),
C API 1.0.0, with Granite `fb178c8080d163419e8d20f10715c61c53c1ec9b`.
The host [build recipe](https://github.com/RamazanKara/Rubylight/blob/6772d4019c0837e372b801637f3bf1b57bea2765/scripts/build_pyrowave.sh)
pins those revisions and three patches, including rejection of short decoder blocks. Any port must
retain that fix: a duplicate block with zero payload length can otherwise leave the parse cursor
stationary indefinitely.

License is not the blocker. Hans-Kristian Arntzen's
[PyroWave MIT license](https://github.com/Themaister/pyrowave/blob/502a3b52a39312ab82c85b1e2fc0e746faee91a4/LICENSE)
and [Granite MIT license](https://github.com/Themaister/Granite/blob/fb178c8080d163419e8d20f10715c61c53c1ec9b/LICENSE)
allow reuse with their copyright and permission notices retained; the host also retains volk's notice.
The Android reference port retains GPL-3.0. No decoder code, prebuilts or new dependencies are vendored
here, and Moonlight's GPL-3.0 license and attribution remain intact.

### What the sources establish

- **Existing mobile implementation:**
  [joemossjr16's Android results](https://github.com/joemossjr16/pyrowave-streaming) report about 5.7 ms
  GPU decode at 1972×1248 on Snapdragon 8 Elite Gen 5 / Adreno 840, with FP16 and the fragment path.
  This is an upstream measurement, not a measurement from this run, and does not certify HDR 4:4:4.
  The inspected [Artemis port at `387d3a5c`](https://github.com/joemossjr16/artemis-android-pyrowave/tree/387d3a5ce1e3df8d4a4d29eec5b81a7b904926a5)
  explicitly refuses HDR in `Game.java`, uses `R8_UNORM` decode planes, and its
  [colour shader](https://github.com/joemossjr16/artemis-android-pyrowave/blob/387d3a5ce1e3df8d4a4d29eec5b81a7b904926a5/app/src/main/jni/pyrowave-renderer/shaders/planar_csc.frag)
  implements eight-bit limited-range BT.709. Importing that renderer unchanged would lose the
  requested precision and misinterpret HDR colour.
- **Vulkan requirements:** the pinned
  [C API](https://github.com/Themaister/pyrowave/blob/502a3b52a39312ab82c85b1e2fc0e746faee91a4/pyrowave.h)
  and [decoder](https://github.com/Themaister/pyrowave/blob/502a3b52a39312ab82c85b1e2fc0e746faee91a4/pyrowave_decoder.cpp)
  require subgroup basic/vote/ballot/arithmetic/shuffle/shuffle-relative operations and compatible
  subgroup size control (wave4–128). Vulkan version alone is insufficient. The reference Android
  probe additionally checks shaderInt16, storageBuffer8BitAccess, timelineSemaphore,
  computeFullSubgroups and synchronization2. Those checks are conservative: upstream
  [common code](https://github.com/Themaister/pyrowave/blob/502a3b52a39312ab82c85b1e2fc0e746faee91a4/pyrowave_common.cpp)
  has a texel-buffer alternative to eight-bit storage when the device's texel-buffer limit is large
  enough; shaderFloat16 is optional. Missing one reference-port feature is not proof that every
  upstream decoder path is impossible. Upstream prefers fragment iDWT on proprietary Qualcomm
  drivers; a compute-only port cannot assume desktop performance on mobile.
- **Current wire format:** host
  [RTSP](https://github.com/RamazanKara/Rubylight/blob/6772d4019c0837e372b801637f3bf1b57bea2765/rust/core/src/rtsp.rs)
  advertises `PYROWAVE/90000` and bitstream ID `186f0393`; selecting it requires
  `x-nv-vqos[0].bitStreamFormat=3`. The current
  [framing code](https://github.com/RamazanKara/Rubylight/blob/6772d4019c0837e372b801637f3bf1b57bea2765/rust/core/src/pyrowave.rs)
  uses a little-endian packet count followed by little-endian lengths and codec packets, or record
  framing when negotiated. This differs from the reference Android renderer's big-endian `PYRW`
  version-1 container. Presence of `pyrowaveAdaptiveFec`, **even with value 0**, selects records;
  `pyrowaveFeatures & 1` also selects them. A port must agree on framing and bitstream revision,
  validate lengths and padding, and integrate with depacketization/FEC before advertising support.
  This client's pinned moonlight-common-c has neither PyroWave negotiation nor its framing path.
- **HDR presentation:** the host
  [conversion shader](https://github.com/RamazanKara/Rubylight/blob/6772d4019c0837e372b801637f3bf1b57bea2765/rust/windows/src/shaders/color.hlsl)
  writes ten-bit codes normalized by 1023 into R16_UNORM planes. Limited-range luma is 64–940;
  chroma is centred at 512 with scale 896. A port needs full-resolution high-precision planes,
  correct BT.2020/PQ and range conversion, and a supported ten-bit HDR surface format/colour-space
  pair. It must also handle the existing HDR control metadata and HDR/SDR transitions. Android
  MediaCodec's `KEY_HDR_STATIC_INFO` does not configure a Vulkan swapchain.
  [Vulkan HDR metadata](https://docs.vulkan.org/refpages/latest/refpages/source/VK_EXT_hdr_metadata.html)
  supplies mastering/content metadata but does not select colour encoding; both paths need validation.

The [compatible protocol definitions](https://github.com/Nonary/moonlight-common-c/blob/d6a11bc685b41037b352a96f29d08276fe5359ba/src/Limelight.h)
use separate client and server namespaces:

| PyroWave profile | Client format | Server capability |
| --- | ---: | ---: |
| Eight-bit 4:2:0 | `0x010000` | `0x00800000` |
| Eight-bit 4:4:4 | `0x020000` | `0x01000000` |
| HDR10 4:2:0 | `0x040000` | `0x02000000` |
| HDR10 4:4:4 | `0x080000` | `0x04000000` |

`NvConnection.negotiateVideoFormats()` now excludes these unimplemented client codec families
before the native handshake. The synthetic Rubylight serverinfo fixture advertises all four
alongside conventional HEVC/AV1 HDR (`0x07830301`). Tests verify AV1 HDR, HEVC HDR, SDR/H.264 fallback,
and that PyroWave HDR/4:4:4 bits do not create conventional HDR/4:4:4 support. Existing native RTSP
still chooses AV1, HEVC or H.264 from the ordinary host markers. No PyroWave setting or capability
is exposed, including on Vulkan-capable Android devices.

### Local measurement limit

No Android emulator or system image is installed in the supplied SDK. A small native Windows
capability probe directly loaded Chrome 154.0.8037.98's **software** `vk_swiftshader.dll`, with its
process restricted to two CPUs; it created a Vulkan 1.3 instance and queried the device, without
creating a decoder or submitting GPU work. DLL SHA-256:
`b08a7477b6e0f2ce9669cfbb8f13e5dd9ce6156d956e15641534489f80c0b40a`.

| Queried property | SwiftShader Device (Subzero) |
| --- | --- |
| Device API | Vulkan 1.3.0 |
| shaderInt16 / storageBuffer8BitAccess / shaderFloat16 | false / false / false |
| timelineSemaphore / synchronization2 | true / true |
| subgroupSizeControl / computeFullSubgroups | true / true |
| Subgroup operations / min–max size | `0xbf` / 4–4 |
| shaderStorageImageWriteWithoutFormat | true |

This fails the existing Android port's conservative feature gate, not a test of all possible
upstream decoder configurations or mobile hardware. There are **zero measured decoded frames**;
decode avg/p95/p99 are **unavailable**, not 0 ms. Milestone 1's `FrameLatencyStats` needs actual
decoder-input and completed-output timestamps. Feeding invented timestamps, timing a rejected
probe or copying the upstream 5.7 ms value into its CSV would not satisfy the requested benchmark.
No such benchmark result is claimed. This environment limitation does not establish mobile
infeasibility and is not a substitute for implementing the missing decoder.

Before enabling PyroWave, real-device testing must cover:

- Adreno and Mali/Immortalis: selected kernel's actual subgroup, storage/texel-buffer, image-format,
  queue and memory limits; unsupported devices must retain HEVC/AV1 negotiation. Validate initialization
  failure, surface replacement, pause/resume, rotation and HDR mode changes without leaking Vulkan objects.
- Ten-bit ramps and fine 4:4:4 colour text on an HDR panel against the host reference: limited/full
  range, BT.2020/PQ, black/white levels, no eight-bit intermediate, mastering/MaxCLL/MaxFALL metadata,
  missing/changed metadata and ten-bit SDR. Verify the actual SurfaceView swapchain and display output.
- Current Rubylight container and record modes, bitstream mismatch, malformed/short/duplicate records,
  packet loss/reorder/FEC and reconnects. Repeat HEVC/AV1/H.264 fallback with stock Sunshine/Apollo hosts.
- Actual frame completions at 720p/1080p and native resolution, 60/120/240 Hz where supported, including
  sustained thermals. Record milestone 1 input-to-output avg/p95/p99 and per-frame CSV using completed
  GPU work; report GPU timestamp duration separately. Present submission is not scanout. Repeat on a
  capable software implementation for correctness, but do not infer phone timing from software or desktop GPU results.

## Milestone 4 verification and hardware follow-up (historical record)

Passed for the milestone 4 exclusion/audit change: `testNonRootDebugUnitTest` (30 tests, no failures),
`assembleNonRootDebug` (ARM64), and `lintNonRootDebug` (zero errors, 194 warnings),
using JDK 17, Gradle `--no-daemon --max-workers=2` and
ndk-build `-j2`. Other ABIs and the root flavor were not built to keep shared-machine load low.
The temporary Gradle init script adds `-j2` to `android.defaultConfig.externalNativeBuild.ndkBuild.arguments`;
`-Pandroid.injected.build.abi=arm64-v8a` restricts this verification build to ARM64.

Local JUnit tests cover launch/resume query construction, fractional/high refresh, exact portrait pixels,
scale, unavailable virtual drivers, Sunshine/Apollo/GFE compatibility, app-list extension fixtures,
codec intersection, exclusion of all four PyroWave profiles, conventional HDR/4:4:4 fallback,
HDR metadata layout and host latency units/percentiles/CSV.
XML fixtures are synthetic protocol examples, not captured live sessions. JUnit and kXML are test-only
dependencies: assertions and a real XML pull parser without an emulator. Build with JDK 17 and the
installed SDK; cap Gradle workers and ndk-build at two. No host build, service, GPU work or live stream
is part of Android build verification; the separate software Vulkan capability query above is not a decode benchmark.

Real-device testing still required:

- Pair two installations, reconnect/resume and unpair; verify distinct host device/display identities,
  existing-pair upgrade behavior and stock Sunshine/Apollo compatibility.
- Request native portrait, landscape and fullscreen sizes at 60/90/120/144+ Hz and fractional 59.94/119.88;
  check selected Android mode, host geometry/rate, 100%/150% render scale, driver-unavailable fallback,
  device/app overrides and display restoration. Host rounds odd dimensions to even; AMD AV1 may pad
  coded dimensions, so inspect edge pixels and cropping on non-aligned native sizes.
- Exercise HEVC and AV1 HDR10 on real HDR panels: ten-bit output, BT.2020/PQ, mastering/content metadata,
  HDR on/off and changed/missing metadata, host ten-bit SDR override, SDR-display and codec-mismatch fallback.
  Check API 24–28 metadata clearing and that HDR stays on SurfaceView when TextureView was selected.
- Exercise actual AVC/HEVC 4:4:4 decoders (HEVC profile requires API 37), fine colored text, size/rate
  capability reporting and 4:2:0 fallback against Radeon and other hosts; verify HDR wins over SDR 4:4:4.
- Compare host processing overlay/CSV with host logs, including absent timing, drops, reconnects and
  sustained high FPS; client render callbacks are not physical scanout measurements.


## Client-side upscaling (2026-10-09)

MediaCodec SDR can output to a zero-copy external-OES SurfaceTexture on a dedicated GLES 3.0
thread. Bilinear and SGSR 1 each draw once; FSR 1.0 runs AMD EASU into a display-sized RGBA8 texture, then RCAS
to the existing EGL window surface. Aspect ratio, Stretch and the SurfaceTexture crop/orientation
matrix are respected. Shaders, uniforms, textures and tracking buffers are prepared at stream
start; the Java draw path allocates no per-frame objects and does no work without incoming frames.
Extra GPU passes can increase battery drain and heat.

The [AMD source](https://github.com/GPUOpen-Effects/FidelityFX-FSR/tree/a21ffb8f6c13233ba336352bdff293894c706575)
is adapted to GLES FP32 with explicit OES taps instead of gather4, native high-precision
arithmetic and finite flat-color divisions. The complete MIT notice is packaged in
[NOTICE-FSR1.txt](../app/src/main/assets/NOTICE-FSR1.txt).

SGSR 1 uses [Qualcomm's mobile RGBA shader](https://github.com/SnapdragonGameStudios/snapdragon-gsr/blob/d926f074bcb9d714e179f1ce0fcb9ee2eeb5074e/sgsr/v1/include/glsl/sgsr1_shader_mobile.frag),
with its 12-tap Lanczos-like filter, green-channel edge detection, 8/255 edge threshold and
23/255 correction limit. Explicit crop-clamped OES taps replace texture gathers; FP32 arithmetic
keeps divisions finite. The SurfaceTexture matrix applies to every sample. No intermediate
texture or RCAS pass is needed. The complete BSD-3-Clause notice, both upstream copyright
years, revision and adaptations are packaged in [NOTICE-SGSR1.txt](../app/src/main/assets/NOTICE-SGSR1.txt).
It works on supported GLES hardware generally; Adreno is Qualcomm's optimization target.
The FP32/OES port needs device measurements; upstream timing numbers are not this app's timings.

Video settings offer Off (default), Bilinear, FSR 1.0 and SGSR 1. SGSR help reads
"Single pass, fastest on Snapdragon/Adreno". The shared sharpening slider previews its numeric
value while dragging. FSR keeps its linear RCAS strength: 0% bypasses RCAS, 100% is maximum.
SGSR maps the same percentage linearly to `EdgeSharpness = 1 + percent / 100`, within
[Qualcomm's documented range](https://github.com/SnapdragonGameStudios/snapdragon-gsr/blob/d926f074bcb9d714e179f1ce0fcb9ee2eeb5074e/sgsr/v1/README.md):
0% keeps spatial reconstruction without extra edge sharpening; 100% uses upstream's value 2.
Changes apply to the next stream. Per-PC profiles save both settings; old profiles inherit
global values. Battery saver selects Bilinear, Low latency selects SGSR, Best quality selects
FSR, and Balanced keeps Off. Existing frame budgets and HDR/PyroWave exclusions apply to all presets.

HDR and all negotiated 10-bit streams, including 10-bit SDR, bypass the 8-bit GPU path.
HDR transitions and unexpected PQ/HLG or P010 output also force direct output. PyroWave retains
its Vulkan renderer and reports that upscaling needs a MediaCodec stream. Unsupported GLES/OES
or EGL timing, shader/codec-surface errors, and size changes use direct output with a visible
reason. Codec recovery also leaves the GPU path to avoid retaining stale texture buffers.
Fallback uses the existing coordinated codec restart: stop the decoder, release the
EGL producer, then configure the original output surface. No host files are changed.

Original presentation deadlines are forwarded through eglPresentationTimeANDROID; frame-rate
votes and Android ARR / fixed-maximum VRR policy stay on the same display surface.
Only one GPU submission is allowed in flight. Completion fences are checked without blocking,
using a reusable callback at 1 ms while work is outstanding. Advanced stats and Copy stats show
an exponential mean of release-to-GPU-completion/swap time, including handoff, swap blocking
and polling delay. This is a conservative added-path estimate, **not measured scanout latency**.
MediaCodec's intermediate-texture callback is excluded from shown FPS and presentation latency;
these remain unavailable on the GPU path. After fallback, stats show direct output at zero
added post-process time and retain the last GPU estimate and reason.

After eight startup samples, six over-budget samples trigger fallback; samples below 75% of
budget reduce the slow count. Three local dropped frames within 60 samples also trigger fallback.
A stall longer than six frame intervals triggers even without completion callbacks. The budget
uses the lower of stream rate and physical panel rate. Drops count only buffers released into
the upscaler and skipped by texture acquisition or a busy GPU, not network loss.
Fallback stays latched until reconnect to avoid oscillation under thermal load.

JVM tests cover enable/bypass decisions, viewport geometry, hysteresis/stalls, bounded timestamp
and drop tracking, AMD EASU golden constants, RCAS strength, SGSR input-size constants and
edge-sharpness mapping, shader-source contracts, mode-array ordering, legacy profiles and presets.
A Robolectric regression covers Off's stream-sized surface buffers and letterbox/stretch layout.
NDK shader checks compile portable arithmetic as ES 3.10 with sampler2D
in place of external OES and validate SPIR-V; this does **not** validate a phone's GLES 3.0/OES
driver. Actual compile/link checks run at stream startup and fall back on failure.

Local verification: JDK 17 / Gradle 9.7.1, `--no-daemon --max-workers=2`:
`:app:assembleNonRootDebug` and `:app:testNonRootDebugUnitTest` passed (**615 tests**, no failures,
errors or skips). `:app:lintNonRootDebug` fails with **603 existing MissingTranslation errors
and 212 warnings**. An isolated, unchanged HEAD (`02c9630d`) lint run reproduces the same
diagnostics; SGSR adds none. The earlier FSR-only note reporting green lint is superseded by
these fresh runs. Translation resources and lint configuration were not changed to hide failures.
A Robolectric regression also checks UI cancellation while the GPU startup/recovery lock is
held and subsequent worker shutdown. SGSR's portable shader compilation and SPIR-V validation
pass; the APK contains all five shaders and both complete licenses. The SGSR shader and notice
were byte-checked against the packaged assets. Verification used the installed Gradle
distribution, worktree-local Gradle/Android user directories and a read-only shared dependency
cache. No adb, emulator or phone was used; Adreno performance and visual quality remain unmeasured.

### Native Android scaling APIs (checked 2026-10-09)

Off sends MediaCodec output directly to the existing SurfaceView. Its holder now uses
`setFixedSize(streamWidth, streamHeight)` for letterboxed streams as well as stretched streams,
while the view still preserves the requested aspect ratio. This makes source-sized buffers
available for compositor/hardware-overlay scaling, without an application GPU post-process.
The optional TextureView retains its source-sized buffer path but is composed into the app window;
it does not provide the same independent hardware-overlay opportunity.
[SurfaceHolder API](https://developer.android.com/reference/android/view/SurfaceHolder#setFixedSize(int,int)),
[AOSP SurfaceView architecture](https://source.android.com/docs/core/graphics/arch-sv-glsv).

Android's baseline compositor scaling is bilinear: AOSP's
[Skia RenderEngine](https://android.googlesource.com/platform/frameworks/native/+/refs/heads/main/libs/renderengine/skia/SkiaRenderEngine.cpp)
uses `SkFilterMode::kLinear` with no mipmaps when texture filtering is needed. Hardware Composer
can perform scaling on a display plane instead; its exact filter and overlay eligibility depend
on the device. There is no public filter-quality selector or guarantee that composition avoids
the GPU on every frame. Off's zero added post-process statistic excludes compositor/scanout work.

| Public surface/API | What it offers and what this client does |
| --- | --- |
| [`SurfaceControl.Transaction`](https://developer.android.com/reference/android/view/SurfaceControl.Transaction) | `setScale` (API 33), crop and position change geometry. `setGeometry` (API 29) is deprecated since API 33. There is no public `setScalingMode` method in this class (also checked against the compile SDK 37 stubs). These calls do not select a reconstruction filter. SurfaceView already manages the layer, so the public holder API is sufficient. |
| [`MediaCodec.setVideoScalingMode`](https://developer.android.com/reference/android/media/MediaCodec#setVideoScalingMode(int)) | API 16 supports scale-to-fit and scale-to-fit-with-cropping. These describe fit/crop geometry, not FSR/SGSR or a selectable decoder super-resolution filter. The renderer already requests `VIDEO_SCALING_MODE_SCALE_TO_FIT` on configuration/recovery; it is also the codec's default after buffer/format changes. |
| [`Display.Mode`](https://developer.android.com/reference/android/view/Display.Mode) | Lists physical resolutions, refresh rates and HDR capabilities. Choosing a mode does not choose an upscaler. Existing display-mode and `Surface.setFrameRate`/ARR handling remain in use. |
| [`DisplayManager`](https://developer.android.com/reference/android/hardware/display/DisplayManager) | Display/HDR capabilities and `getHdrConversionMode` (API 34) describe output/color management. `WindowManager.LayoutParams.setHdrConversionEnabled` controls HDR conversion, not spatial scaling. No public scale-quality hint enables OEM reconstruction for a decoded Surface. HDR/10-bit still bypass the 8-bit shaders. |
| [Android 16 ADPF headroom](https://developer.android.com/about/versions/16/features#headroom-apis) | `SystemHealthManager.getCpuHeadroom`/`getGpuHeadroom` (API 36) estimate device load on supported hardware. They neither upscale pixels nor measure this pass's latency. Existing optional decoder performance hints are retained; SGSR uses the same measured completion fences and one-frame budget instead of adding an unvalidated headroom policy. |

Android 15+ does not provide a public Android SDK/NDK switch for OEM super resolution on arbitrary
app surfaces. [Samsung Game Booster](https://www.samsung.com/uk/support/apps-services/updates-to-game-booster-settings-and-features-on-the-samsung-galaxy-devices/)
offers user/OEM-controlled per-game resolution and frame-boost settings. Qualcomm's
[Adreno Frame Motion Engine](https://www.qualcomm.com/processors/adreno) is a vendor frame-generation
feature, distinct from spatial SGSR. Neither has a documented public Android surface API that
this client can enable; that conclusion is based on the public API inventory, not an assertion
that all private/vendor integrations are impossible. Newer Qualcomm
[Adreno Neural Fusion SDK](https://www.qualcomm.com/developer/blog/2026/09/introducing-adreno-neural-fusion-sdk-for-snapdragon-mobile-platforms)
integration is an engine/Vulkan pipeline with jitter and motion-vector requirements, not a
compositor scaling hint suitable for this MediaCodec path.
[Game Mode interventions](https://developer.android.com/games/optimize/adpf/gamemode/gamemode-interventions)
can downsize an app backbuffer under OEM policy (Android 12 on select devices, Android 13+);
that is not a public choice of high-quality scaler. No hidden APIs, vendor parameters or
device-global settings are used here.

Real-device checklist (not run here):

1. Compare Off, Bilinear, FSR and SGSR at 720p/1080p → native: small text, diagonal edges, fine textures,
   motion and black/white ramps, with sharpening 0/50/100%. Check ringing, banding, range/color,
   orientation and crop. Repeat letterboxed, stretched and portrait output. Native/larger input
   should report direct output.
2. Save Advanced and copied stats per mode at 60/120 FPS and higher where supported. Record added
   milliseconds and independently measure presentation/latency with a device trace or camera.
   GPU-path shown FPS/presentation fields must remain unavailable instead of counting decoder
   texture callbacks as display presentation.
3. Repeat VRR off/on with changing game cadence, both Android ARR and fixed-maximum fallback,
   SurfaceView and optional TextureView. Check physical/render Hz, frame-rate hints, pacing,
   judder/tearing and whether the display queue grows.
4. Run a thermal/battery comparison for at least 20 minutes and deliberately overload the GPU.
   Verify one visible fallback reason, released GPU resources, resumed direct output, and no
   reactivation until reconnect. A single short spike must not repeatedly toggle renderers.
5. Enable HDR10 and host 10-bit SDR, and compare PQ highlights, gradients and metadata against
   Off. Both GPU modes must report direct output with the HDR/10-bit note. Toggle host HDR live.
   Repeat with PyroWave and missing GLES/OES support; check the stated direct-output reason.
6. Exercise disconnect/reconnect, background/resume, PiP, rotation/resize, surface destruction
   during startup/fallback and codec recovery. Check hangs, black/stale surfaces and leaked
   threads/buffers/EGL objects. Verify per-PC save/reset, legacy inheritance, presets, slider
   cancellation/live value, and copied stats after fallback.
