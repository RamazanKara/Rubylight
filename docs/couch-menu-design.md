# Couch menu design

Status: design, 2026-10-26. Nothing is implemented. The feature ships as **Experimental**: we cannot test iOS, tvOS, Steam Deck, Android TV or webOS clients on our own devices.

The couch menu is a host menu that Rubylight draws into the video stream. Any Moonlight client can open it with a controller, a keyboard or touch, with no client UI and no protocol change: stock Moonlight on every platform, Artemis and Nightjar. The menu reuses the host's existing logic for Radeon settings with restore, the stream preset, session actions and live GPU readings. It also has a slot for each sibling feature's toggle.

Naming: code, config and the native API say `couch_menu`. Text on screen says "Host menu" ("Host-Menü" in German).

## 1. Decisions at a glance

| Question | Decision |
|---|---|
| Controller chord | **L3+R3 (both stick clicks) held for 1.0 s**, with no other button held, on any one pad. No client we checked intercepts it (section 3). |
| Keyboard | **Ctrl+Alt+Backspace** opens, and the same chord or Esc closes. |
| Touch and mouse | **Press and hold for 1.5 s in the top-left corner box**, which is 6% of the stream's shorter side. Sources are touch, pen, absolute mouse, or relative mouse at the host cursor. |
| Native | `POST /api/native/v1/session/couch-menu {"open":bool}`, advertised by the feature `session.couch_menu`. This is additive to native API v1; `PROTOCOL_VERSION` does not change. |
| Where the input filter runs | One pure core filter on `&[Input]`, placed after the permission check and before `Injector::apply_all` (GameStream) and before `Sink::apply` (WebRTC). |
| Game input when the menu opens | `Injector::release_all()` (or `Sink::release`) runs once. While the menu is open, every game event is consumed. After it closes, buttons and keys that are still held stay masked until the player lets go of them. |
| Where it draws | A **second, rect-sized dispatch** in the same D3D12 compute command list. It uses its own PSO and root signature and runs only while the menu is open. The D3D11 graphics fallback gets one extra pixel-shader draw per plane. The closed path keeps the same PSOs, the same command list and one dispatch. |
| Panel | Opaque, so the menu pass never reads the capture. Text is drawn in target colour space: sRGB, BT.2020 SDR, or PQ at the SDR white level. The panel goes through each format's own writer code, so NV12, P010, packed 4:4:4 v1 and v2, and PyroWave 4:2:0 and 4:4:4 are all handled. |
| Static desktops | A new menu revision forces one repeat encode of the last image, as a recovery does. While open, the `static_period` repeats continue unchanged. |
| Text | Embedded Spleen bitmap font (BSD-2-Clause), 8×16 and 16×32, Latin-1. The CPU draws the panel once per change at 1×. The shader scales it up by a whole number with nearest sampling. |
| Strings | `couch_menu/*` keys in `rust/assets/locale/en.json` and `de.json`, looked up by the host's `console::i18n`. `locale` = `de*` gives German; every other locale gives English (v1). |
| Permissions | Exactly the native API rules: Disconnect needs View or Launch; End needs Launch; Close game needs Launch plus `host_control`; every Radeon write, restore and preset change needs `host_control`. A locked row is shown with its reason, never hidden. |
| Preset change | Saved as the device's `config_overrides.stream_preset` through the console's own validation. It applies **from the next stream**, and the row says so. A live encoder rebuild is deferred until it has been measured. |
| Nightjar | An "Open host menu" item, gated by the native feature, never by host brand. No new native toggles: Nightjar already has native Radeon and session panels. Each sibling feature brings its own native setting in its own design. |
| Closed cost | No new GPU work, upload, PSO, descriptor or thread. Each frame does one atomic load. Each input event does one bitmask compare. |
| Open cost | Shown per frame in a new `overlay` column of the timing record, and per 5 s in `menu_frames`/`menu_uploads` on the `stream timings` line. |

## 2. Goals and non-goals

Goals:
- Every Moonlight client can reach the host's controls in a stream.
- The closed path is unchanged and costs no latency.
- Failures are loud.
- No logic is duplicated: every action calls the code the native API already uses.

Non-goals for v1:
- A translucent or blurred backdrop. The panel is opaque so that the pass never reads the source.
- Locales other than EN and DE.
- Opening the menu from input-only peers.
- Applying a preset live.
- The readback encode path (`wgc_direct_encoder_input = false`): opening is refused loudly there (section 6.6).

## 3. Opening and closing

### 3.1 What clients intercept

The chords below were checked against the client sources: moonlight-qt `gamepad.cpp`, moonlight-ios `ControllerSupport.m`, and Nightjar `ControllerHandler.java` and `protocol/desktop/src/gamepad.rs` / `stream/mod.rs`.

| Combination | Who takes it | Usable? |
|---|---|---|
| Start+Select+L1+R1 | Every Moonlight client: quit. Nightjar uses it for its stream menu. | No |
| Start held over 750 ms | moonlight-qt, Moonlight Android and Nightjar desktop: toggles mouse emulation when Start is released, whatever else is held | No hold involving Start |
| Start+Select | iOS and Android on pads without a Home button: turned into Guide. Nightjar desktop: its local menu, consumed. | No |
| Select+L1+R1+X | moonlight-qt: stats overlay | No |
| Select+PS / Select+LB | moonlight-qt and Android: clickpad emulation | No |
| LB+Start, Start+RB | iOS and Android: Select or Guide emulation on pads with one button | No |
| Guide | Android TV home, Steam Deck Steam button, iOS (by default), Xbox Game Bar on the host when pressed | No (not by default) |
| Ctrl+Alt+Shift+key | moonlight-qt and Nightjar desktop shortcuts | No |
| 2- and 3-finger taps | Every touch client: right click, keyboard | No |
| **L3+R3** | Nobody | **Yes** |
| **Ctrl+Alt+Backspace** | Nobody among the clients. Windows does not reserve it. Under X11 it would zap the server only if `DontZap` is off, which has not been the default since 2009. | **Yes** |

**Keyboard combos the clients keep** (checked 2026-10-26 against moonlight-qt `app/streaming/input/keyboard.cpp` and `input.cpp`, and moonlight-android `Game.java` `handleSpecialKeys` and `KeyboardTranslator.java`):
- moonlight-qt consumes a key only while Ctrl, Alt and Shift are all down, and then only its shortcut keys (Q, Z, X, S, M, C, D, V, L, E, K). Every other combo is translated and sent. Without system key capture (the windowed default), the client's OS keeps Alt+Tab, Alt+F4, the Windows key and Ctrl+Alt+Del.
- moonlight-android consumes Ctrl+Alt+Shift with Z, Q or C and eats key-downs until the modifiers are up. Every other key with a mapping is sent; Backspace, Home, End and Insert all have one.
- Outside the clients: Windows keeps Ctrl+Alt+Del on the client PC, Linux desktops keep Ctrl+Alt+F1..F12 (VT switch), Ctrl+Alt+T, Ctrl+Alt+L and Ctrl+Alt+arrows, and AltGr is Ctrl+Alt on Windows, so Ctrl+Alt with a letter types a character on many layouts (AltGr+M is µ in German).

So `couch_menu_keyboard_chord` offers a fixed list of Ctrl+Alt combos with keys that type nothing: `ctrl+alt+backspace` (default), `ctrl+alt+home`, `ctrl+alt+end`, `ctrl+alt+insert`. Anything with Ctrl+Alt+Shift is refused with its own message. A client running inside a Remote Desktop session would lose Ctrl+Alt+End and Ctrl+Alt+Home to the RDP client; the default is unaffected.

### 3.2 Rules (core `couch_menu::chord`)

All time is the host's receive time (`Instant`). A chord that is pending sets a deadline. The control loop wakes on that deadline through `next_due`, so a hold completes without new packets, because Moonlight sends controller state only when it changes.

**Controller.**
- A pad's `buttons` becomes exactly the chord (`couch_menu_controller_chord`, default `LS_CLK | RS_CLK`). Sticks and triggers are ignored.
- That arms the chord for that pad. Any other button cancels it, and so does releasing a chord button.
- After `couch_menu_hold_ms` (default 1000 ms) the chord fires. The pad then waits for a button change before it can arm again; a pad whose hold was cancelled by an open or close waits the same way, so a second pad still holding the chord does not fire a second later.
- A single button held alone is a valid chord and follows the same rules.
- The game has already seen the chord's presses. That is acceptable; the alternative is buffering, which adds latency.
- `PadButtons::parse` refuses a chord containing all of one set Moonlight keeps, with one message per set, most specific first: Start+Select+L1+R1 (quit), Start+Select (becomes Guide), Start (a 750 ms hold toggles mouse mode, so no hold may include Start), Select+L1+R1+X (stats), Select+Guide and Select+L1 (moonlight-qt and Moonlight Android turn both into a clickpad press).
- It also refuses A, B and each D-pad direction, one message each, because `Filter::pad_nav` reads them while the menu is open: holding a chord with A to close the menu would press Accept on the focused row, and Disconnect has no confirm step.
- `guide` parses, with a warning that many clients hand Guide to the OS and that a Guide press reaching the host opens Xbox Game Bar. A chord with Select gets a warning (`OpenWith::select_conflict`, from `Config::update` and `OpenWith::from_config`) when `back_button_timeout` is in `[0, couch_menu_hold_ms)`: the host's Back button emulation presses Guide first.

**Keyboard.**
- Fires on a key-down of the chord's key (`couch_menu_keyboard_chord`, default VK_BACK 0x08) with the Ctrl and Alt modifier bits set and Shift clear. `KeyChord::parse` takes the modifiers in any order.
- The key event that fires it is consumed.
- Only the first key-down after the key was up counts: auto-repeats of the held key neither toggle the menu again nor go Back.

**Touch and mouse.**
- A contact, pen tip or mouse button goes down inside the corner box, in stream coordinates.
- It is held for `couch_menu_corner_hold_ms` (default 1500 ms) without leaving the box.
- Positions come from `Touch`, `Pen` and `Absolute` events.
- For `Relative` mice the filter asks the host for the cursor's stream position only when the hold timer fires (`Injector::pointer_in_stream()`), so no position is read otherwise.

**Config.** `couch_menu_open_with` is a list, default `controller,keyboard,touch`. Removing an entry turns that path off. `OpenWith` carries it and the four chord and hold settings (section 8); `OpenWith::from_config` reads them all, and a hand-edited value that `Config::update` would refuse is logged once and its default used. `Config::update` checks each through `chord::check`.

Closing uses the same controller or keyboard chord, B or Esc on the top page, the Back to game row, a tap outside the panel, or `POST {"open":false}`. The menu also closes, with no input, when the session stops.

### 3.3 Input while open (core `couch_menu::filter`)

`Filter::apply(now, &mut Vec<Input>, pointer) -> Outcome { release_game, nav: Vec<Nav> }`

Closed:
- Events pass through unchanged and in order.
- The filter only reads each event to watch for a chord.

Opening, whether by chord or by the native API:
- `release_game = true`, so the host calls `release_all()` once.
- The triggering events are removed.
- When the native API opens the menu, the open flag is set in `Shared`. The control loop is woken through its existing wake event, and the filter notices the change on its next pass.

Open:
- Every event is consumed except `Arrival` and `Battery`. Those always pass, so pads that join are still created.
- A `Controller` event whose `active` mask differs from the last one the game saw passes with its buttons, triggers and sticks at rest, so a pad unplugged while the menu is open leaves the host too.
- Events turn into `Nav` actions:
  - **Pad:** D-pad and the left stick (above 50% deflection, back below 30%) give Up, Down, Left and Right. Holding repeats after 400 ms, then every 120 ms. A accepts, B goes back, and the chord closes.
  - **Keyboard:** the arrow keys, Enter or Space to accept, Esc or Backspace to go back. The keyboard chord closes.
  - **Mouse:** the wheel moves up and down. A left click on a row selects and accepts it, using the absolute position or the host cursor. A right click goes back.
  - **Touch:** a tap on a row selects and accepts it. A tap outside the panel closes the menu.
  - `Text`, `Motion` and `ControllerTouch` are consumed.

Closing:
- The filter records what is still held: pad buttons per pad, keys and mouse buttons.
- Later `Controller` events have those bits masked until each bit drops.
- Key-ups and button-ups for presses the game never saw are dropped.
- The game never sees a half press.

Ordering:
- Permission checks (`client.allows(event.required_permission())`) run before the filter.
- A device without controller permission therefore cannot open the menu with a pad.

Input-only peers (`Role::InputOnly`) get no filter.

## 4. Menu content

### 4.1 Pages

Main page:
1. **Status line**, read-only: GPU temperature, power, clock and load (up to four metrics, in `hardware_metrics_selection` order), plus the stream mode (for example `2560x1440 120 Hz HEVC 80 Mb/s`). It refreshes once a second while open. With `hardware_metrics = false` the line says "GPU readings are off (hardware_metrics)".
2. **Back to game**.
3. **Radeon settings ▸**, if the `radeon.read` feature applies. Its page lists `current_view(h)`:
   - Left and Right step through each setting. Writes go through the journal.
   - **Restore Radeon settings** calls the restore path.
   - Settings that are unsupported or read-only are shown in grey with their state.
   - An active `radeon_profile` is named on one line.
4. **Stream preset**: Lowest latency, Balanced or Reference quality.
   - It shows the running stream's preset.
   - Left and Right change it, which needs `host_control`.
   - The note "Applies from the next stream" is always shown.
5. **Extension rows** from the registry (section 4.3). None exist until a sibling feature registers one.
6. **Disconnect (game keeps running)**.
7. **End session**, which asks to be pressed again to confirm within 5 s.
8. **Close game**, which asks the same and needs `host_control`.

Errors appear in red on a status line at the bottom.
- The text is "Failed: <message>", using the `Failure` message.
- Failure messages are in English even when the menu is in German.
- The status line stays until the next action.
- While an action runs, the row says "Working..." and further accepts are ignored.
- Every failure is also logged at `warn` with the action and the error code.

### 4.2 Reuse map (no duplicate logic)

| Row | Calls | Change needed |
|---|---|---|
| Disconnect, End, Close game | `session_control::{disconnect_session, end_session, close_game}` with the session's client uuid, re-read from `h.paired` for every action. Each runs on the menu worker; End and Close can block for up to 5 s. | none |
| Permission check | The same `require` and `require_host_control` rules as `native_api.rs` | make the helpers `pub(crate)` |
| Radeon read, write, restore | `native_api/radeon.rs` | Split `write_settings(h, connection, body)` and `restore_settings` into `write_for(h, &Client, SettingsWrite)` and `restore_for(h, &Client)`. The HTTP handlers and the menu both call those. |
| GPU readings | `telemetry.rs`, the hardware-metrics worker | add `sample_blocking(h, client)` for the worker thread |
| Preset | `/api/clients/update` in `web/clients.rs` (with `stream_preset::check_overrides`) | Extract `set_device_override(h, uuid, key, value)` and use it in both places |
| Strings | `console/i18n.rs` | add `pub fn lookup(locale, path) -> Option<String>` |

### 4.3 Extension registry (`rust/host/src/couch_menu/items.rs`)

```rust
pub(crate) trait Item: Sync {
    fn key(&self) -> &'static str;              // "audio_night_mode"
    fn label(&self) -> &'static str;            // catalogue path "couch_menu/items/audio_night_mode"
    fn available(&self, h: &Host, s: &Session) -> Availability; // Hidden | Locked(reason) | Ready
    fn value(&self, h: &Host, s: &Session) -> ItemValue;        // Bool | Choice(index, &[catalogue paths])
    fn set(&self, h: &Host, s: &Session, client: &Client, v: ItemValue) -> Result<(), Failure>;
    fn needs_host_control(&self) -> bool;
    fn applies(&self) -> Applies;               // Live | NextStream (row shows the note)
}
pub(crate) static ITEMS: &[&dyn Item] = &[];    // each sibling feature adds one line
```

Expected entries:
- `audio_night_mode`: Bool, Live.
- `audio_headphone_surround`: Bool, Live.
- `encoder_focus`: Bool or Choice; Live if its design rebuilds nothing, otherwise NextStream.
- `sleep_after_idle_minutes`: Choice of Off, 15, 30, 60 or 120; needs host control; Live.

Each item's `set` calls that feature's own setter, so the menu holds no feature logic. The tests use a fake item.

## 5. Host architecture

Per session, `Arc<couch_menu::Shared>` lives in core `Session` (field `couch_menu`). It holds:

```rust
pub struct Shared {
    state: AtomicU64,                 // bit 63 open, low 63 bits revision
    frame: Mutex<Option<Arc<Frame>>>, // published panel: origin, size, scale, rgba (Arc<[u8]>)
    wake: Mutex<Option<Box<dyn Fn() + Send + Sync>>>, // stream loop wake (capture_wake.set())
    control_wake: Mutex<Option<Box<dyn Fn() + Send + Sync>>>, // control loop wake
}
```

- **Control thread.** It holds the `Filter` per `ControlPeer` and sends `Nav` to the worker over an `mpsc` channel. `next_due` is folded into the existing `due` computation.
- **Menu worker thread.** It is spawned on the first open, one per session, and then parked on `recv` while closed. It owns the `Menu` model:
  1. Builds a `Snapshot` (rows, values, availability) from the reuse map.
  2. Runs actions and renders with core `render`.
  3. Stores the `Frame`, raises the revision, and calls `wake`.
  4. Polls GPU readings once a second, only while open.
  5. Exits when the session stops.
- **Video thread** (stream loop):
  1. Once per iteration: `let st = shared.state.load(Acquire)`.
  2. Only if `st != last_st`: it locks `frame`, calls `encoder.set_overlay(...)`, and sets `menu_dirty`.
  3. `menu_dirty` counts as `recovering` in the repeat path (stream.rs, about line 2530). A still picture is encoded again at once; with arrival pacing, at the next grid slot. A revision change also calls `set_repeat(false)`, so PyroWave treats the frame as fresh content.
  4. Closing (open bit clear) sets `set_overlay(None)` and forces one more repeat, so a static desktop drops the panel.

## 6. Drawing

### 6.1 Where

All three capture paths reach the encoder as a `GpuImage` through the converter: the IDD push ring (virtual displays), WGC, and DDX (lock screen, UAC and the secure desktop). Drawing at the converter therefore covers every capture path, the lock screen, both encoders (AMF and PyroWave) and WebRTC, which shares the stream loop.

The single hook is `Encoder::set_overlay(Option<Arc<windows::overlay::Overlay>>)`, next to `set_luminance` and `set_repeat`.

### 6.2 D3D12 compute (`compute.rs`, `shaders/color.hlsl`)

**Shader.**
- Refactor each compute entry body into a function that takes a literal `bool menu`:
  - `yuv420_body(uint3 id, bool menu)`;
  - `chroma_pack_body` (packing v1 and v2);
  - `pyro_body`.
- Each body gets a pixel's colour from `rgb(float2 t, bool menu)`, which returns `menu ? menu_rgb(t) : nonlinear(t)`.
- fxc inlines the functions and folds the literal, so the existing entries keep their code.
- New entries:
  - `yuv420_menu_cs`, `chroma_pack_menu_cs` and `pyro_menu_cs` add `menuOriginBlocks` to `id` and return beyond `menuBlocks`.
  - The thread-to-block mapping is already in target-block units: 2×2 for yuv420, pyro and pack v2, 2×1 for pack v1. The packed regions and the co-sited chroma are therefore written by the same code.

**Resources.**
- `Texture2D<float4> menuImage : register(t2)` holds RGBA8 sRGB at 1×.
- `menu_rgb(t)` loads `menuImage[clamp((t - origin) / scale)]`.
- A clamp at the rect's edges keeps the chroma filter's left neighbour inside the panel. The panel has a 2 px border of one colour, so no seam shows.

**Colour, after the panel colour `c`.** `c` is sRGB; `rgb()` returns it in the space `nonlinear()` returns:

| Stream | Value returned |
|---|---|
| SDR, `colorMatrix != 2` | `c` |
| SDR BT.2020 | `bt2020_sdr(gamut2020(srgb_linear(c)))` |
| HDR | `pq(gamut2020(srgb_linear(c)) * sdrWhiteScale / 125)`, i.e. panel white at the configured SDR white level, matching the cursor |

Range, 10-bit and PyroWave codes all come from the existing `lumaCode`, `chromaCode`, `pyroLuma` and `pyroChroma`.

**Pass.** It is recorded in `convert()` and `convert_planes()` after the main `Dispatch` and only when an overlay is set:
1. A UAV barrier (null resource).
2. `SetPipelineState(menu PSO)` and `SetComputeRootSignature(menu root)`. The menu root has CBVs b0 and b2, an SRV table at t2 (1 descriptor), and the same UAV range: 2 for AMF, 3 for PyroWave.
3. b0 is the slot's colour region, as in the main pass. b2 (`cbuffer Menu`, `Overlay::constants`: `menuOrigin`, `menuSize`, `menuOriginBlocks`, `menuBlocks`, `menuScale`) points at a second 256-byte region per slot, so the constant buffer grows from `SLOTS×256` to `SLOTS×512`. A separate cbuffer, rather than more `Config` fields, keeps the closed entries' bytecode byte-identical.
4. `Dispatch(ceil(menuBlocks / 8))`.

The menu PSOs and root are created on the first open, and that first-open cost is logged.

**Upload.**
- A ring of 3 DEFAULT textures and one UPLOAD buffer per converter.
- When the revision changes: `CopyTextureRegion` in the same command list, before the main dispatch. A texture is reused only after the slot fences that read it have completed.
- Each slot keeps its texture alive in `slots[slot].inputs`, as it does the pointer.
- The cache is keyed on (revision, converter), so a rebuilt converter uploads again.

### 6.3 D3D11 graphics fallback (`gpu_color.rs`, `amf/gpu.rs`)

This path runs when textures cannot be shared, after two encoder failures, for AMF yuv444, and for the PyroWave graphics path.
- After each plane's normal draw, when an overlay is set, it draws a full-plane triangle with the matching `*_menu` PS: `menu_luma`, `menu_chroma`, `menu_chroma_pack_y`, `menu_chroma_pack_uv`, `menu_pyro_y`, `menu_pyro_u`, `menu_pyro_v`, `menu_pyro_444`.
- Each `*_menu` PS calls `discard` when its target pixel is outside the rect.
- The PS entries are the existing bodies with `menu = true`.
- `PSSetShaderResources(2, menu)` and `PSSetConstantBuffers(2, menu)` (the 48-byte `cbuffer Menu`, beside `Config` at b0). `UpdateSubresource` runs only on a new revision.

A full-plane pass while open is the price of keeping the packed layouts in one code path. When closed, the only change is a CPU branch.

### 6.4 Layout and scale (core `couch_menu::layout`)

`s = min(width, height)` of the encoder target:

| s | Font | Scale | Line |
|---|---|---|---|
| < 720 | 8×16 | 1 | 16 px |
| 720 to 1439 | 16×32 | 1 | 32 px (4.4% at 720p, 3% at 1080p) |
| 1440 to 2159 | 16×32 | 2 | 64 px |
| ≥ 2160 | 16×32 | 3 | 96 px |

- The panel is centred, 34 columns wide, with as many rows as the page needs (at most 12 rows plus a title, a status line and a hint).
- Its origin and size are rounded to multiples of 16 target pixels, the strictest block size.
- If the panel does not fit at the chosen scale, the scale drops by one. If it does not fit at 8×16 ×1, opening is refused with the log line `couch menu: stream too small (WxH)`.
- Portrait streams use the same rule.

### 6.5 Text and atlas (core `couch_menu::font`, `render`)

- Spleen 2.x `spleen-8x16.bdf` and `spleen-16x32.bdf` are converted once by `font/convert_bdf.py`, a dev tool that is not part of the build.
- The output is 1 bpp, Latin-1 (U+0020 to U+00FF) and about 14 KB per size, committed and loaded with `include_bytes!`.
- The license file and its source SHA-256 sit next to it, and `NOTICE` gains a line.
- The converter fails if any glyph is missing. A core test asserts that every character in the EN and DE `couch_menu` strings, every digit and `°%./:-` are in both atlases.
- Rendering is crisp 1-bit text on an opaque panel:
  - panel `#17171C`;
  - text `#F0F0F0`;
  - disabled `#8A8A93`;
  - focus bar in ruby `#E0115F`;
  - error `#FF5A5A`.
- A page is drawn into a 1× RGBA8 `Frame` in well under 1 ms, only on a change.
- The menu strings must be Latin-1: "..." and never "…". A test checks this.

### 6.6 Paths the menu does not draw on (loud)

- **`wgc_direct_encoder_input = false`.** `encode()` reads the frame back to the CPU, so the overlay cannot be composited there.
  - The chord logs `ERROR couch menu: needs wgc_direct_encoder_input` once per session.
  - The native API answers `temporarily_unavailable` with that message.
  - `Encoder::encode()` returns an error if an overlay is set, so an overlay is never dropped silently.
- **`Role::InputOnly`.** These sessions have no video, so neither the filter nor the feature applies. The native API answers `temporarily_unavailable`.

## 7. Native API and Nightjar

### 7.1 Native API (additive; crate `rubylight-native-api` 1.0.0 to 1.1.0)

- `feature::COUCH_MENU = "session.couch_menu"`. It is advertised when `couch_menu = true` (the default) and the caller may see sessions.
- `POST /api/native/v1/session/couch-menu`, body `CouchMenuRequest { open: bool }` with `deny_unknown_fields`. It returns `CouchMenu { open: bool }`.
- Rules:
  - The caller needs View or Launch and must own a `streaming` session.
  - Otherwise the reply is `no_session`, `not_session_owner` or `permission_required`.
  - The reply is `temporarily_unavailable` when the feature is off, on the readback path, or for an input-only session.
  - The call is idempotent.
- Old clients ignore the new feature. Stock Moonlight never sees any of it.

### 7.2 Nightjar

**"Open host menu".**
- The item appears in the Android `StreamMenu` and in the desktop stream menu.
- It is shown only when the native capabilities list `session.couch_menu`. It is gated by capability, never by brand.
- Choosing it closes Nightjar's own sheet first, so input flows to the stream again. The POST then runs on the native menu thread.
- A failure is shown as a toast with the API message.

**Pass-through.** Nightjar must keep forwarding L3+R3 and Ctrl+Alt+Backspace unchanged, which it does today. Regression tests pin this:
- Android `ControllerHandler`, including Android TV key mapping (`KEYCODE_BUTTON_THUMBL`/`THUMBR`);
- desktop `gamepad.rs`;
- desktop `stream/mod.rs` key handling.

**Native toggles.** No new ones in this work. Nightjar already exposes Radeon and session actions natively. Sibling features add native endpoints in their own designs, and Nightjar's native panel follows those.

## 8. Config and labelling

| Key | Default | Meaning |
|---|---|---|
| `couch_menu` | `true` | Master switch. When false there is no filter, no feature and no worker. |
| `couch_menu_open_with` | `controller,keyboard,touch` | Which ways open the menu |
| `couch_menu_controller_chord` | `l3+r3` | The pad chord, a `+`-joined button list (section 3.2). Per device and app. |
| `couch_menu_hold_ms` | `1000` | The pad hold, 300..=3000 ms. Per device and app. |
| `couch_menu_corner_hold_ms` | `1500` | The corner hold, 500..=3000 ms. Per device and app. |
| `couch_menu_keyboard_chord` | `ctrl+alt+backspace` | One of `chord::KEY_CHORDS` (section 3.1). Per device and app. |

All six keys are listed in `console/settings.rs` under the label "Couch menu (Experimental)". The console fields (Brief 3): the controller chord is a text field that shows `Config::update`'s refusal message as written, and the `PadButtons::warning` text for a chord with Guide; the two holds are number fields with the ranges above; the keyboard chord is a select over `KEY_CHORDS`. The device editor offers the four chord and hold keys as overrides. `docs/configuration.md` gets a "Couch menu (Experimental)" section, the release notes say Experimental, and the README gets no caveats.

## 9. Measurement

**Closed (the hard rule).**
1. **Structure.** The closed path issues exactly one `Dispatch` and no copy. A test-only dispatch counter in the converter checks this. GPU fixture outputs must be bit-identical to hashes recorded on main before the shader refactor, for every format and colour mode (Brief 2).
2. **A/B.** Compare release builds of the base SHA and the feature on this PC, only when the user is idle.
   - Run 60 s quick arms with `BUTTERPOLLO_TIMING_RECORD` set: A/B/A/B/A/B, the same client (lap `smoke.ps1`), 1440p120 HEVC.
   - `timing_report.py` reports the p50 and p99 of present to first sent, and of claimed to submitted.
   - Pass: each arm's difference is within the A/A spread, and `overlay` is 0 on every row.
   - Any regression beyond the spread blocks the merge.

**Open.**
1. In one session, toggle the menu every 10 s for 60 s with `POST /session/couch-menu`. Use `curl` with the harness's paired test certificate.
2. Run `timing_report.py --split overlay`. It reports claimed to submitted (CPU record and submit), submitted to encoded (GPU convert plus encode), and present to first sent, grouped as 0 = closed, 1 = open, 2 = open with upload.
3. The `stream timings` 5 s line gains `menu_frames=N menu_uploads=M`, but only when N > 0, so closed lines keep their exact format.

Report the numbers measured at 1080p and 2160p, SDR and HDR, AMF and PyroWave. This document claims no budget. If the open cost is visible at p50, try the cheaper variant first: a dispatch over the rect only, which is already the design, and shrink the panel.

## 10. Tests (summary; the full lists are in the briefs)

- **Core.** Chord timing at 999 ms and 1000 ms, an extra button cancelling, multiple pads, the keyboard chord, corner hold for each source, leaving the box cancelling, release masking after close, nav repeat, model permissions and confirm flow, layout table and alignment, a renderer golden hash per page, atlas coverage, and `next_due() == None` while closed and idle.
- **Windows.**
  - Overlay validation and dispatch rect maths (pure).
  - GPU tests (`#[ignore]`): closed bit-identity; inside the rect matching a CPU reference within ±1 code; outside the rect identical to closed; uploads only on a new revision. These run for the D3D12 and D3D11 paths, NV12/P010/pack v1 and v2/pyro 420 and 444, SDR709/BT2020/PQ.
- **Host.**
  - Filter wiring through the existing control-loop test fixture: the chord leads to `release_all` exactly once.
  - The WebRTC fake sink sees release and no game events while open.
  - A static picture is encoded again within one period of a new revision.
  - Native endpoint matrix (permissions, owner, input-only, off).
  - Every action through fakes (the `RadeonBackend` fake, the session fixtures).
  - Locale keys complete in `en` and `de`.
  - Timing column.
- **Nightjar.** Item gating, the POST path and body, error mapping, and the chord pass-through tests.

## 11. Open risks

1. **The L3+R3 hold reaches the game.** Some games bind it, for example a special ability. The 1 s hold makes accidental triggers rare, but the game sees the press. Mitigations: `couch_menu_open_with`, and a future per-app off switch.
2. **The touch path on stock clients.** Moonlight Android in absolute mode may turn a long press into a quick right click instead of a held button, and in trackpad mode the result depends on the host cursor. Test on the Retroid with stock Moonlight Android and Nightjar. If a path does not hold, drop it from the default and document it. iOS cannot be tested and stays Experimental.
3. **Desktop side effects.** The press in the top-left corner reaches the desktop before the menu opens, for example starting a drag of the Recycle Bin. `release_all` ends the drag in place. Acceptable, but it should be checked.
4. **Shader refactor and the closed path.** If fxc emits different code for the existing entries, the closed path changes. The bit-identity hashes and the A/B are the gate. If either fails, generate the menu entries from macros instead of a shared body.
5. **The D3D11 fallback costs a full-plane pass while open.** This is acceptable only because it is a fallback; measure it.
6. **Stock Moonlight after Disconnect or End from the menu.** Check that the graceful code (`disconnect_now(0)`) shows no error dialog on moonlight-qt and Android.
7. **Glyph coverage.** Spleen Latin-1 coverage must be confirmed at conversion, and the converter fails loudly. Terminus (OFL) is the fallback font.
8. **Overlapping files with sibling designs.** `encoder_focus` may touch `amf.rs`, `encoder.rs` or `compute.rs`, so sequence it with Brief 2 rather than running both at once.
9. **Bitrate.** Sharp text on a low-bitrate stream may look soft for a frame or two after each change. That is the encoder's rate control at work, not a menu bug.
10. **Changing the preset live** needs an encoder rebuild, which means an IDR and a short stall. It is deferred until measured. Users may expect the change to take effect at once, so the row's note must stay visible.

## 12. Implementer briefs

Order: Briefs 1 and 2 run in parallel (disjoint files). Brief 3 runs after both are on main. Brief 4 runs after Brief 1's native-api change is pushed. Each brief runs in its own worktree under `C:\src`. Never use `git stash`.

The quick loop for each brief is `C:\src\rl-quick.ps1 -Repo <worktree> -Crates <crates> -TargetDir <own dir>`, then grep `C:\src\rl-quick.log`. One full `build.ps1` gate runs per batch, done by the integrator.

### Brief 1: core menu, filter, renderer, atlas, native-api types (implementer, about 90 turns)

**Owns** (all new unless marked):
- `rust/core/src/couch_menu/mod.rs` (`Shared`, `Frame`, `Nav`, `Outcome`)
- `rust/core/src/couch_menu/chord.rs`
- `rust/core/src/couch_menu/filter.rs`
- `rust/core/src/couch_menu/model.rs` (pages, rows from a host-provided `Snapshot`, focus, confirm, `Action` out)
- `rust/core/src/couch_menu/layout.rs`
- `rust/core/src/couch_menu/render.rs`
- `rust/core/src/couch_menu/font.rs`
- `rust/core/src/couch_menu/font/spleen-8x16.bin` and `spleen-16x32.bin`
- `rust/core/src/couch_menu/font/LICENSE-spleen.txt` (with source URL, version and SHA-256)
- `rust/core/src/couch_menu/font/convert_bdf.py`
- `rust/core/src/lib.rs` (one `pub mod` line)
- `rust/core/native-api/src/lib.rs` (`feature::COUCH_MENU`)
- `rust/core/native-api/src/session.rs` (`CouchMenuRequest`, `CouchMenu`)
- `rust/core/native-api/Cargo.toml` (version 1.1.0)
- `Cargo.lock` (that version line only)
- `NOTICE` (append the Spleen credit; keep everything else intact)

**Steps.**
1. Implement sections 3.2, 3.3, 4.1 (model only; the host fills `Snapshot`), 6.4 and 6.5 as pure code with no OS calls.
   - `Filter` takes `now: Instant`. When a relative-mouse hold fires, it calls a `FnMut() -> Option<(f32, f32)>` for the pointer.
   - `Filter::next_due()` returns `None` when closed and nothing is armed.
   - `render(&Menu, &Snapshot, &Layout) -> Frame`: RGBA8 sRGB, alpha 255 inside the panel, with a 2 px border of one colour.
   - `Shared` packs open and revision into one `AtomicU64`.
2. Native-api types with serde round-trip tests. Old `Capabilities` JSON must still parse.

**Acceptance.** All of these as unit tests:
- chord at 999 ms and 1000 ms, per pad, an extra button cancelling;
- keyboard chord with and without Shift;
- corner hold for each source, leaving the box cancelling, relative-mouse lookup made only when the timer fires;
- events identical and in order while closed (property test over random `Input` streams with no chord);
- `release_game` exactly once per open; consume set while open;
- masking after close until release; no orphan ups reach the game;
- nav repeat timings;
- model: locked rows keep their reason, the confirm window is 5 s, back and close;
- layout table rows (360p, 720p, 1080p, 1440p, 2160p, portrait 1080×2400; 640×360 fits at 8×16 ×1, since the largest panel is 288×272, and anything below that such as 320×240 is too small), 16-px alignment;
- golden FNV hash per page at 720p and 2160p;
- atlas covers every char of the catalogue strings (pass the strings as test data copied from section 6.5's rule, since the catalogue lives in host).

**Test.**
- `cargo +1.98.1-x86_64-pc-windows-msvc test -p butterpollo-core --locked --target-dir C:\src\rl-target couch_menu`
- `cargo +1.98.1-x86_64-pc-windows-msvc test -p rubylight-native-api --locked --target-dir C:\src\rl-target`
- `C:\src\rl-quick.ps1 -Repo <wt> -Crates butterpollo-core,rubylight-native-api -TargetDir C:\src\rl-target-menu-core`

**Cost check.** A test asserts that `Filter::apply` while closed does not allocate and leaves `next_due` at `None` (count allocations with a test allocator or `Vec` capacity checks).

### Brief 2: overlay pass in converters and encoders (implementer, about 100 turns; parallel with Brief 1)

**Owns:**
- `rust/windows/src/overlay.rs` (new): `Overlay { revision, origin: [u32; 2], size: [u32; 2], scale, rgba: Arc<[u8]> }`, `validate(target_size)`, the CPU reference YUV for tests
- `rust/windows/src/lib.rs` (mod line)
- `rust/windows/src/shaders/color.hlsl`
- `rust/windows/build_shaders.rs`
- `rust/windows/src/compute.rs`
- `rust/windows/src/gpu_color.rs`
- `rust/windows/src/amf/gpu.rs`
- `rust/windows/src/encoder.rs` (`set_overlay`)
- `rust/windows/src/amf.rs`
- `rust/windows/src/pyrowave.rs`
- `rust/windows/src/input.rs` (`Injector::pointer_in_stream() -> Option<(f32, f32)>` from `GetCursorPos` over the stream output's rect, known from `geometry`)

**Steps.**
1. **Before any change**, add the GPU fixture test that hashes the closed outputs for every format and colour mode on main, and commit the hashes.
2. Implement 6.2 and 6.3 exactly:
   - the shared bodies with `bool menu`;
   - the new entries;
   - the menu root, PSOs (created lazily) and upload ring;
   - the second dispatch only when an overlay is set;
   - the D3D11 menu PS draws.
3. `Encoder::set_overlay` stores the overlay. AMF and PyroWave pass it to their converters. `encode()` errors when an overlay is set.

**Acceptance.**
- Closed hashes unchanged, with exactly one `Dispatch` per frame while closed (test-only counter).
- Inside the rect matches the CPU reference within ±1 code for every format, mode and path (NV12, P010, pack v1 and v2, pyro 420 and 444; SDR709, BT2020 SDR, PQ; D3D12 and D3D11). Outside the rect matches closed bit for bit.
- Same revision means no upload; a new revision means one upload; a rebuilt converter uploads again.
- The textures in the ring are not reused before their fence.
- Overlay validation rejects a rect that is unaligned or out of bounds.

**Test.**
- `C:\src\rl-quick.ps1 -Repo <wt> -Crates butterpollo-windows -TargetDir C:\src\rl-target-menu-gpu`
- The GPU tests need the same environment that `rl-quick.ps1` sets up. Copy its environment lines into the shell, check that the user is idle, then run `cargo +1.98.1-x86_64-pc-windows-gnu test -p butterpollo-windows --locked --target-dir C:\src\rl-target-menu-gpu overlay -- --ignored --test-threads=1`.

**Cost check.** The closed-path hashes, the dispatch count and an unchanged `stage_convert_ms` are the evidence. Report the GPU-test wall time with and without an overlay as a first signal only. Release measurement belongs to Brief 3.

### Brief 3: host wiring, actions, native endpoint, timing, docs (implementer, about 120 turns; after Briefs 1 and 2)

This brief may exceed 500 lines. If it does, run it as two sequential halves on the same agent: **3a** covers video, overlay and timing; **3b** covers input, actions, the API and docs.

**Owns:**
- New files:
  - `rust/host/src/couch_menu.rs` (worker, Snapshot, actions, i18n, Frame → `windows::overlay::Overlay`)
  - `rust/host/src/couch_menu/items.rs` (registry, empty, with a test fake)
  - `rust/host/src/nvhttp/native_tests/couch_menu.rs`
- `rust/host/src/main.rs` or `lib.rs` (mod line)
- `rust/host/src/stream.rs`:
  - control loop: filter per `ControlPeer`, `release_all` on open, fold `next_due` into `due`, control wake;
  - video loop: state load, `set_overlay`, `menu_dirty` in `recovering`, `set_repeat(false)` on a new revision, `menu_frames`/`menu_uploads` on the timings line, the overlay value into the timing record.
- `rust/host/src/webrtc/input.rs` (filter before `Sink::apply`, release on open)
- `rust/host/src/native_api.rs` (route, feature, `pub(crate)` permission helpers)
- `rust/host/src/native_api/radeon.rs` (`write_for` and `restore_for` extraction)
- `rust/host/src/telemetry.rs` (`sample_blocking`)
- `rust/host/src/web/clients.rs` (`set_device_override` extraction)
- `rust/host/src/console/i18n.rs` (`lookup`)
- `rust/host/src/console/settings.rs` (two settings)
- `rust/host/src/nvhttp/native_tests.rs` (mod line)
- `rust/core/src/session.rs` (`couch_menu` field)
- `rust/core/src/timing_record.rs` (append the `overlay` column: 0, 1 or 2; the magic stays `1` because `DictReader` readers are unaffected)
- `rust/host/src/timing_record.rs`
- `rust/assets/locale/en.json` and `de.json` (`couch_menu` section, Latin-1)
- `rust/release/timing_report.py` and `rust/release/test_timing_report.py` (`--split overlay`)
- Docs:
  - `docs/configuration.md` (section "Couch menu (Experimental)")
  - `docs/native-api.md` (endpoint, feature, errors)
  - `docs/timing-record.md` (column)
  - `docs/architecture.md` (one paragraph in the frame path)
  - `docs/rubylight-3.0-plan.md` (status table row)
  - `rust/RELEASE_NOTES.md` (`## Unreleased`, Experimental)

**Steps.** Implement sections 3.3 (wiring), 4.2, 4.3, 5, 6.6 and 7.1. Every action calls the extracted shared functions; nothing is reimplemented.

**Acceptance.**
- The fixture control loop with a chord calls `release_all` exactly once, forwards no game event while open, and masks after close.
- The WebRTC fake sink behaves the same way.
- A still picture is encoded again within one `period` of a new revision, and once after close.
- The endpoint matrix: View or Launch owner OK; other device `not_session_owner`; no session `no_session`; feature off, input-only or readback gives `temporarily_unavailable`; idempotent.
- The capability is listed only when enabled.
- The actions work through the `RadeonBackend` fake and the session fixtures, and each permission-locked row carries its reason.
- Every `couch_menu/*` key exists in `en` and `de` and is Latin-1.
- The timing record has 15 columns, and `test_timing_report.py` passes with old 14-column files.
- `couch_menu = false` creates no worker, no filter and no feature (test).

**Test.**
- `C:\src\rl-quick.ps1 -Repo <wt> -Crates butterpollo-core,butterpollo -TargetDir C:\src\rl-target-menu-host`
- `python -m unittest discover -s rust/release -p test_timing_report.py`

**Open controls, notes for the wiring.**
- A `Filter` cannot swap its `OpenWith` while it runs, so an override changed during a session needs a setter on the filter or a new filter.
- `apply_overrides` drops `""` values, so a device cannot set `couch_menu_open_with` to "nothing"; it has to turn the `couch_menu` master switch off.
- The console must send `couch_menu_open_with` as a comma-joined string, not a JSON array.

**Cost check, hardware (runner, user idle, isolated profile on port 48123).** Run the closed A/B and the open toggle run of section 9 against a release build, and append the numbers to `rust/PERFORMANCE.md`. Exit code 3 from the harness means stop and investigate; never retry.

### Brief 4: Nightjar, "Open host menu" and pass-through tests (implementer, about 70 turns; after Brief 1 is pushed)

**Owns** (in `C:\src\rubylight-android`):
- `protocol/client/Cargo.toml` (bump the `rubylight-native-api` rev to the Brief 1 commit)
- `protocol/Cargo.lock`
- `protocol/client/src/http/native.rs` (`couch_menu(open) -> Result<CouchMenu, Error>`)
- `protocol/android/src/native_api.rs` (`"couch_menu"` call)
- `app/src/main/java/com/limelight/nativeapi/NativeApiClient.java`
- `app/src/main/java/com/limelight/nativeapi/NativeCapabilities.java` (`COUCH_MENU = "session.couch_menu"`)
- `app/src/main/java/com/limelight/ui/StreamMenu.java` (item `HOST_MENU`)
- `app/src/main/java/com/limelight/Game.java` (handler: dismiss the sheet, POST on the native menu thread, toast on error)
- `app/src/main/res/values/strings.xml` and `app/src/main/res/values-de/strings.xml` ("Open host menu" / "Host-Menü öffnen")
- `protocol/desktop/src/session_page.rs` (or the desktop stream menu that lists session actions)
- `protocol/desktop/src/strings/de.rs`

**Tests:**
- `app/src/test/java/com/limelight/ui/StreamMenuTest.java`
- `app/src/test/java/com/limelight/nativeapi/NativeApiClientTest.java`
- `app/src/test/java/com/limelight/binding/input/ControllerHandlerTest.java`
- the test modules in `protocol/desktop/src/gamepad.rs`, `protocol/desktop/src/stream/mod.rs` and `protocol/client/src/http/native.rs`

No preference keys are renamed or added.

**Acceptance.**
- The item is shown only with the native feature, and never because of brand or `Family`.
- The POST goes to `/api/native/v1/session/couch-menu` with `{"open":true}`, and API errors keep their code.
- The Nightjar sheet closes before the POST.
- Android: L3+R3 held for 2 s reaches `sendControllerInputPacket` unchanged, through the `KEYCODE_BUTTON_THUMBL`/`THUMBR` path, and opens no Nightjar menu.
- Desktop: L3+R3 returns no `Action` and forwards `LS_CLK|RS_CLK`. Ctrl+Alt+Backspace is forwarded and not consumed.

**Test.**
- `.\gradlew.bat :app:testDebugUnitTest --tests com.limelight.ui.StreamMenuTest --tests com.limelight.nativeapi.NativeApiClientTest --tests com.limelight.binding.input.ControllerHandlerTest --offline`
- `cd protocol; cargo test -p nightjar-desktop gamepad; cargo test -p nightjar-desktop session_page; cargo test -p nightjar-core native`

**Cost check.** Not applicable; the client does no work for the menu beyond one POST.

### After Briefs 3 and 4: hardware acceptance (runner; user idle; phone parts by the user)

Run this on this PC as the host, from Nightjar on Android, from stock Moonlight Android on the Retroid, and from Nightjar desktop and moonlight-qt on the lap.

Open the menu with each path, at 720p, 1080p and 2160p, in each of these modes:
- SDR and HDR;
- AMF HEVC and AV1;
- PyroWave;
- packed 4:4:4 on;
- the WebRTC path (Experimental).

Check:
- the menu is readable;
- a static desktop updates on every move;
- Win+L, then the lock screen menu works;
- Disconnect and End show no client error;
- held buttons do not stick after close.

Record the measurements from section 9. A path that fails on stock clients is dropped from the default and documented.
