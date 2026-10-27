# Wake-on-LAN, sleep after idle, and couch audio

Design, 2026-10-26. H0 and H1 (the host's wake and sleep) are built; H2, H3 and the clients are not yet. The briefs at the end run in the order they give: H0 first, then H1 and H2 in parallel, then H3, then N1a and N1b.

## Summary

| Topic | Decision |
|---|---|
| `<mac>` in `/serverinfo` | The MAC of a physical adapter, chosen by a pure policy in core. Never a virtual or VPN adapter's MAC. Zeros only when no physical adapter is connected, and for unpaired requests as today. |
| More MACs | `GET /api/native/v1/wake` (feature `wake`) lists every physical adapter with its readiness and subnet broadcasts, for Nightjar. Stock Moonlight keeps the single `<mac>`. |
| NIC settings | Always read (as the service, SYSTEM). Changed only with consent: an unchecked installer option, `--wake-on-lan` on a quiet install, or the console's Fix button. Never through the native API. |
| Wake grace | After a resume, while a paired client is polling, the host holds a system-required power request for up to 5 minutes. Otherwise Windows' unattended-sleep timeout (2 minutes by default) puts the PC back to sleep before the stream starts. Ships only if the problem reproduces on this PC. |
| Sleep after idle | `sleep_after_idle_minutes`, off by default. S3 PCs only in this release; Modern Standby PCs report "not supported" loudly. |
| Night mode | `audio_night_mode`: a linked feed-forward compressor, a slow leveller and a soft clipper, all without lookahead. |
| Headphone surround | `audio_headphone_surround`: when a stereo client streams System audio, the sink opens as 7.1 and 8 channels are captured. They're rendered to binaural stereo with SADIE II Neumann KU100 HRIRs (Apache-2.0). The wire stays stereo Opus. |
| Added latency | 0 samples for both: no buffering and no lookahead. Compute time only, estimated at 0.01 to 0.05 ms per 5 ms packet. The ship bound is under 0.25 ms per packet, measured. |
| Settings levels | Host, then device, then app. Then a session choice through launch parameters `audioNightMode` and `audioHeadphoneSurround`. Then a live change for this session through `POST /api/native/v1/audio/settings`. |
| `PROTOCOL_VERSION` | Unchanged. The only new wire pieces are optional launch parameters, which other hosts ignore, and native API endpoints. |

Both features work with every Moonlight client:

- Stock clients get the right `<mac>`, NIC fix-up, wake grace and sleep after idle.
- Stock clients also get both audio settings at host, device and app level.
- Nightjar adds multi-MAC wake, a wait while the PC wakes, and per-PC and in-stream audio toggles. All of it is gated by host capabilities.

## Part A: Wake-on-LAN and sleep after idle

### A.1 What exists today

- `rust/windows/src/net.rs` `local_mac(address)` returns the 6-byte address of the adapter that owns the request's local IP. It doesn't filter anything.
  - A client that reaches the PC through a Hyper-V vEthernet, ZeroTier or another TAP adapter gets a virtual MAC that no packet can wake.
  - A Wintun route (Tailscale, WireGuard) has no MAC and fails, which gives zeros.
- `rust/host/src/nvhttp.rs` (around line 322) caches the MAC per local address in `serverinfo_cache` and sends zeros to unpaired requests. `rust/host/src/webrtc/discovery.rs` (line 93) also calls `local_mac`.
- Nothing reads or changes NIC wake settings, sleep states or power requests. The only power call is `DisplayAwake` (`rust/windows/src/timing.rs`): `ES_CONTINUOUS | ES_DISPLAY_REQUIRED` while capturing. The service handles no power broadcasts.
- Moonlight clients ignore an all-zero MAC and keep the last good one. In Nightjar this is `ComputerDetails.update`, line 239.

What this PC showed on 2026-10-26 (read-only checks):

- **Adapters:**
  - The Realtek 8125 2.5GbE `Ethernet` is up, with `*WakeOnMagicPacket=1`, `S5WakeOnLan=1`, `*ModernStandbyWoLMagicPacket=1`, `*WakeOnPattern=0` and `MSPower_DeviceWakeEnable` true.
  - The MediaTek RZ608 Wi-Fi is disconnected, with wake off.
  - `vEthernet (WSL (Hyper-V firewall))` is up, with a `00:15:5D` Hyper-V MAC and `HardwareInterface=False`.
- **Power:** S3 is available. Hibernate is off, so Fast Startup is inactive even though `HiberbootEnabled=1`. There's no Modern Standby.
- **Reading pitfalls:**
  - `powercfg` output is localized (German here), so nothing may parse it.
  - Without elevation, `Get-NetAdapterPowerManagement` fails with a misleading "device not functioning" error, and `MSNdis_DeviceWakeOnMagicPacketOnly` is access-denied. Reads must therefore run in the service as SYSTEM.

### A.2 Which MAC the host reports

The adapter facts come from three sources:

- `GetAdaptersAddresses`.
- `GetIfEntry2` for each adapter. From `MIB_IF_ROW2` it uses `InterfaceAndOperStatusFlags.HardwareInterface`, `ConnectorPresent`, `PhysicalMediumType`, `PermanentPhysicalAddress`, `OperStatus`, `InterfaceGuid` and the interface metric.
- The IPv4 prefixes, which give the subnet broadcasts.

A pure function in `butterpollo_core::wake` decides:

1. **Physical or virtual:** an adapter is physical when it has `HardwareInterface` and a 6-byte MAC that is neither zero nor multicast. vEthernet, TAP, Wintun, loopback and tunnel adapters count as virtual.
2. **Wired route:** when the route adapter is physical and wired, report its MAC.
3. **Wi-Fi route:** when the route adapter is physical Wi-Fi, report a connected wired physical adapter on the same IPv4 subnet if there is one. Wired wake is far more reliable, and the broadcast reaches it. Otherwise report the Wi-Fi MAC, labelled Experimental.
4. **Virtual route:** when the route adapter is virtual and its MAC equals a physical adapter's permanent MAC (a Hyper-V external switch sharing the NIC's address), report that MAC. Otherwise report the best connected physical adapter, in this order: wired before Wi-Fi, wake-armed (from cached readiness, when there is any) before not, lower interface metric, lower index.
5. **Nothing connected:** zeros.

`local_mac` keeps its signature and output format, and calls the policy. `nvhttp.rs` and `webrtc/discovery.rs` don't change.

A MAC is reported even if its adapter isn't wake-ready. The user can fix the settings later, and zeros would only make clients keep an older value.

### A.3 Readiness and fix-up

Per physical adapter:

| Check | Source | Fix (only with consent) |
|---|---|---|
| Allow this device to wake the computer | WMI `root\wmi` `MSPower_DeviceWakeEnable.Enable`, instance by PnP id | Set to true |
| Wake on magic packet | Keyword `*WakeOnMagicPacket` = 1 (the adapter's advanced property) | Set to 1 (restarts the adapter) |
| Only allow a magic packet to wake | WMI `MSNdis_DeviceWakeOnMagicPacketOnly` | Set to true. Recommended: stops network chatter from waking the PC, which matters with sleep after idle. |
| Wake on pattern | `*WakeOnPattern` | Information only |
| Wake from Modern Standby | `*ModernStandbyWoLMagicPacket`, when the keyword exists and the PC uses Modern Standby | Set to 1 |
| Vendor wake from shutdown (Realtek `S5WakeOnLan` and similar) | The keyword, when present | Information only: each vendor defines its own meaning |

System facts:

- `GetPwrCapabilities`: `SystemS3`, `AoAc` and `HiberFilePresent`.
- `HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Power\HiberbootEnabled`. Fast Startup counts as active only when hibernate is available.
- Waking from shutdown also needs firmware settings that Windows can't read: Wake on LAN or Power on by PCI-E, with ErP or deep sleep off. The console always shows them as "check the BIOS", never as ready.

How the host reads and changes them:

- **One script:** a PowerShell script embedded in `butterpollo-windows` has two modes, `read` and `apply`. It runs in the service with `-NoProfile -NonInteractive -ExecutionPolicy Bypass` and takes its script on stdin.
- **Locale-independent JSON:** it prints JSON keyed by InterfaceGuid, PnP id and registry keyword, never by display names. Core parses and judges that JSON, so fake-NIC tests are plain fixture files.
- **Caching:** results are cached for 60 s, and the console's Check again refreshes them.
- **Blocked script:** if PowerShell is blocked (Constrained Language Mode, AppLocker), the check reports `unknown` with the error.
- **Applying:**
  - Advanced properties are set with `Set-NetAdapterAdvancedProperty ... -NoRestart` per keyword, then one `Restart-NetAdapter` for each changed adapter. This drops the network for a few seconds.
  - WMI changes use `Set-CimInstance`.
  - Turning Fast Startup off sets `HiberbootEnabled=0`. It's a separate choice, offered only while Fast Startup is active.

How consent is given:

- **Installer:**
  - A new optional step after the USB/IP step: "Set up Wake-on-LAN for <adapter names>". Its checkbox starts unchecked. It names each change and warns that the network drops for a few seconds.
  - The answer is remembered in the profile, the way `rust/setup/src/usbip.rs` does it, so an update neither asks again nor re-applies.
  - A quiet install changes nothing unless `--wake-on-lan` is passed.
  - Setup doesn't link `butterpollo-windows`. After copying files it runs the installed `butterpollo.exe --apply-wake-settings`. A failure shows a dialog and goes into the setup log, but doesn't fail the install.
- **Console:**
  - A Wake-on-LAN readiness check on the Overview reports ok, warning or error per adapter and for the system.
  - A Fix button (admin) opens a dialog that lists every change and the network drop.
  - The POST returns 409 while any session is active or pending. Otherwise it returns 202, applies about a second later, and the console polls for the result.

### A.4 Wake grace

Windows puts a PC that woke without local input back to sleep after its "System unattended sleep timeout", 2 minutes by default. Browsing the app list after waking the PC can take longer than that.

1. After a resume, while a paired client has made an HTTPS request in the last 60 s, the host holds a `PowerCreateRequest`/`PowerSetRequest(PowerRequestSystemRequired)` request. Its reason reads "A device is connecting to Rubylight after a wake", and `powercfg /requests` shows it.
2. The request ends when a stream starts, 5 minutes after the resume, or 60 s after the last client request.
3. The host detects a resume without power broadcasts: the gap between `GetTickCount64`, which counts sleep, and `QueryUnbiasedInterruptTime`, which doesn't, grows by more than 30 s between two ticks.
4. First prove the problem exists on this PC: wake it from the NUC, don't connect, and watch whether it goes back to sleep. If it doesn't, drop A.4.

### A.5 Sleep after idle

Both settings are host-wide and can't be overridden per device or app:

- `sleep_after_idle_minutes`: 0 means off and is the default. Otherwise 5 to 1440.
- `sleep_while_suspended`: default false.

A pure state machine, `butterpollo_core::wake::IdleSleep`, gets a snapshot every 30 s and answers `Stay(reason)` or `Sleep`. It answers `Sleep` only when all of these hold:

1. **Feature usable:** the setting is on, the PC has S3 and it isn't a Modern Standby PC. Otherwise the status is `unsupported` with the reason, logged once at startup and again when the setting changes.
2. **No session:** no session is active or pending (`h.sessions.active` and `.pending`, any role or transport), and `h.session_snapshot().state` is Idle. A suspended session, or an app the host launched that is still running (`h.current_app`), blocks sleep unless `sleep_while_suspended` is on.
3. **Long enough:** N minutes have passed since condition 2 was last false, since the host started and since the last resume.
4. **No local input for N minutes.** The service runs in session 0, so it can't see input itself.
   - A helper, `butterpollo.exe --idle-seconds`, runs in the console user's session through `Process::spawn(Target::User { elevated: false })`, as `--playnite-install` does.
   - The helper's exit code is the idle time in seconds, computed as `GetTickCount() - LASTINPUTINFO.dwTime` with u32 wrapping and capped at 604800. A failure exits with `0x8000_0000 | error`.
   - A failed helper blocks sleep, and the status shows the error.
   - With nobody signed in, idle time counts from the last session change.
5. **Nobody keeps Windows awake.** `CallNtPowerInformation(SystemExecutionState)` contains none of `ES_SYSTEM_REQUIRED`, `ES_DISPLAY_REQUIRED` or `ES_AWAYMODE_REQUIRED`.
   - This is meant to cover `SetThreadExecutionState` and power requests (media playback, downloads, games). The host itself holds none at that point.
   - **H1 step 1 must prove that a power request from another process shows up here.** If it doesn't, stop. Undocumented `GetPowerRequestList` as SYSTEM is an architect decision, not something to improvise.
6. **No other activity:** no update is downloading or installing, and no authenticated console request arrived in the last N minutes.

When `IdleSleep` says `Sleep`:

1. The host checks everything again while holding `h.launch_transition`.
2. It logs at warn level what it is doing and why.
3. It enables `SeShutdownPrivilege` and calls `SetSuspendState(FALSE, FALSE, FALSE)`.

A failure is logged as an error and shown in the console. The host tries again at the next full interval, not on every tick.

The console shows the status, plus the time and reason of the last sleep. The status is one of:

- off;
- armed, with a countdown;
- blocked, with the reason;
- unsupported.

Modern Standby: Windows documents no call a service can use to enter Modern Standby; `SetSuspendState` targets S3. In this release these PCs are reported as not supported. They're never silently skipped.

### A.6 Native API `GET /api/native/v1/wake`

Feature `wake`. Any paired device with List, View or Launch can read it. Paired clients already see `<mac>`; `docs/privacy.md` names the additional adapters. MACs are upper case with colons, like `/serverinfo`'s `<mac>`.

```json
{
  "revision": 1,
  "primary": "04:7C:16:B4:13:4F",
  "targets": [
    {
      "mac": "04:7C:16:B4:13:4F",
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

- `targets[].kind`: `ethernet` or `wifi`.
- `targets[].state`: `ready`, `needs_setup`, `unknown` or `experimental` (Wi-Fi).
- `checks[].state`: `ok`, `warning`, `error` or `unknown`.
- `idle_sleep.state`: `off`, `armed`, `blocked` or `unsupported`.
- Unknown fields and values are ignored, the way the crate already ignores them with `Lenient`.
- The MAC format matches serverinfo's.

### A.7 Nightjar

Gaps found:

- **Android, `WakeOnLanSender.java`:**
  - It sends to 255.255.255.255, each interface's broadcast and the known addresses, on ports 9 and 47009 plus 47998, 47999, 48000, 48002 and 48010 (offset by the HTTP port), in 3 bursts.
  - It doesn't use port 7, more than one MAC, or IPv6 all-nodes `ff02::1`.
- **Android, waiting:** the console paths already wait and show "Waking PC…" (`HostWaker`, `WakeLaunch`: resend every 8 s, give up after 90 s). These paths still send once and don't wait:
  - `computers/HostActions.java:302`
  - `ShortcutTrampoline.java:119`
  - `preferences/HostStreamSettings.java:455`
  - `console/ConsoleHomeActivity.java:561`
- **Desktop, `protocol/desktop/src/wol.rs`:**
  - It sends to 255.255.255.255 and the resolved addresses.
  - It doesn't use subnet-directed broadcasts, port 7 or more than one MAC.
  - It shows "Wake request sent" and doesn't wait.

Changes:

- **Sending, both platforms:**
  - Add port 7 to the fixed ports.
  - Send to every known MAC: serverinfo's, plus the `/wake` targets.
  - Add subnet broadcasts: the client's own interfaces, plus the `broadcasts` from `/wake`.
  - Add `ff02::1` on each IPv6 interface. This is best effort: a send error there isn't a failure.
- **Storing targets, Android:** the extra targets go in a new SharedPreferences file, `wake_targets`, keyed by host UUID. The `ComputerDatabaseManager` schema and existing keys stay unchanged. The targets refresh whenever the native capabilities include `wake`.
- **Waiting:**
  - Android: every wake path goes through `HostWaker`. A shortcut waits for the PC, then launches.
  - Desktop: a `WakeWait` state machine ports `WakeLaunch`. It sends at once, resends every 8 s, times out at 90 s, and finishes when the host poll says the PC is online. It shows "Waking <PC>…" with Cancel.
- **Gating:** only the extra targets need `wake` in the native features. Ports, broadcasts and the wait apply to every host.

### A.8 Tests

**Core:**

- MAC policy table tests from adapter fixtures:
  - this PC with anonymized MACs;
  - a Hyper-V external switch;
  - a Tailscale-only route;
  - a laptop with Wi-Fi only;
  - two wired NICs;
  - nothing connected.
- Readiness judgements from recorded `read` JSON.
- The `IdleSleep` machine with a fake clock. Cases:
  - never sleeps with a session;
  - sleeps with a suspended session only when opted in;
  - timers reset on resume;
  - local input blocks;
  - power requests block;
  - an update blocks;
  - console activity blocks;
  - a failed helper blocks;
  - Modern Standby and no S3 report unsupported.
- The wake-grace machine.

**Windows (`#[ignore]`, run on this PC as SYSTEM):**

- Reading the facts finds the Realtek adapter and S3.
- A child process holding `PowerSetRequest(PowerRequestSystemRequired)` makes `SystemExecutionState` show `ES_SYSTEM_REQUIRED`.
- `--idle-seconds` returns a plausible value.

**Host:** the wake tick runs against a fake `PowerSystem` trait. `sleep()` is called exactly once when due, and never with an active, pending or suspended session.

**Hardware:** only with the user's OK, the user away and the host idle.

1. Set a 5-minute sleep on this PC. It goes to sleep.
2. `ssh nuc` sends a magic packet to the reported MAC. Record the time until serverinfo answers.
3. Check the wake grace (A.4) in the same run.

## Part B: couch audio

### B.1 What exists today

`Stream::audio` (`rust/host/src/stream.rs`, starting at line 3006):

- It runs `effective_config` and opens the route: `Route::acquire(&config, &h.directory, s.launch.host_audio, s.config.audio_channels)`, then `route.set_channels(audio_channels)` and `Loopback::new_sink(audio_channels, &sink)`.
- `Loopback` (`rust/windows/src/audio.rs`) maps the endpoint's mix format to `audio_channels` with `mix_matrix`. The stereo downmix drops LFE. It then resamples to 48 kHz with `Resampler`, which handles 2, 6 or 8 channels.
- `audio_sink::collect` returns whole packets of `48 * audio_packet_ms` frames. A packet is 5, 10 or 20 ms, 5 ms by default. Silence is `vec![0.; frames * audio_channels]`.
- Each packet then goes through `opus.encode(samples)` and `output.send`, which feeds both GameStream and WebRTC.
- The Opus layout is chosen from the client's channel count (`OpusLayout::select`). Game-only audio is captured in stereo, since process loopback is fixed at 48 kHz float stereo.
- Problems surface through `s.launch.warnings.set(key, message)`.

### B.2 Insertion point and cost

A new `butterpollo_core::audio_dsp::Chain` processes each packet in place, between `collect` and `encode`:

```rust
for (samples, at) in packets.iter_mut().zip(positions) {
    let samples = chain.process(samples); // capture_channels in, audio_channels out
    for _ in 0..output.send(&opus.encode(samples)?, now, at)? {
        loss.unsent();
    }
}
```

The chain keeps per-channel state across packets. It never holds back or waits for a sample, and its output doesn't depend on packet size (a test checks this).

**Added latency**

- Both processors add 0 samples of buffering and 0 of lookahead. The only wall-clock cost is compute time.
- Night mode: estimated at about 0.01 ms per 5 ms packet.
- Headphone surround: 14 FIRs of 256 taps is 3,584 multiply-adds per output frame, about 172 million per second. Vectorized, that's an estimated 0.02 to 0.05 ms per 5 ms packet on this PC.
- These are estimates. H2 measures them with an `#[ignore]` release bench.
- Hard bound: under 0.25 ms per 5 ms packet (5% of a packet) on this PC and on the 780M laptop. If it's slower, the change doesn't ship.

**Other properties**

- **Both off:** `Chain` passes the packet through untouched, and the output is bit-identical (tested).
- **Binaural onset:** the filters are trimmed to their common onset. The near-ear path of the front pair peaks within 8 samples (0.17 ms), and the generator asserts this. That offset is the HRTF's own arrival-time cue, not a buffer.

### B.3 Night mode

`audio_night_mode`: a boolean, off by default. Its parameters are fixed; there are no user knobs in this release.

- **Detector:** peak, linked across all channels (the largest |x| on any channel). Because every channel gets the same gain, the spatial image and the LFE balance stay put.
- **Compressor:**
  - Threshold -30 dBFS, ratio 3:1, 6 dB soft knee.
  - Gain is smoothed in dB with a 3 ms attack and a 250 ms release.
- **Leveller:**
  - A slow gain toward -23 dBFS RMS, measured with a 3 s one-pole RMS. As in BS.1770, the power leaves out the LFE and is normalized to a stereo pair, so the same programme reads the same in 2, 6 or 8 channels.
  - Range -6 to +12 dB, slewing at no more than 3 dB/s.
  - It holds while the input RMS is below -60 dBFS, so silence and noise floors aren't boosted.
- **Soft clipper:** memoryless. Unity below -3 dBFS, then a smooth approach to 0.98. It catches the attack overshoot that a compressor without lookahead lets through.
- **Turning it on or off:** turning it on starts from unity gain, or during the off ramp from the ramp's current gain. Turning it off ramps the gain to unity over 10 ms within the packet, then bypasses. Neither direction clicks.
- **Bad samples:** NaN or infinite input samples become 0. The stream gets one session warning, `audio_bad_samples`, naming the problem.

### B.4 Headphone surround

`audio_headphone_surround`: a boolean, off by default.

**When it is active**

All of these must hold:

- it is on when the stream starts;
- the client asked for stereo (`s.config.audio_channels == 2`);
- `stream_audio` is on;
- capture uses System audio.

**What changes when it's active**

- **Capture:** `Route::acquire` and `set_channels` ask the sink for 8 channels, and `Loopback::new_sink(8, ...)` captures 8. A 5.1 endpoint fills the 8-channel frame through `mix_matrix`.
- **Wire:** the Opus layout stays stereo, so nothing changes on the wire and every Moonlight client works.
- **Stereo endpoints:** a new `Loopback` accessor returns the endpoint's source channel count and mask. With a stereo endpoint, such as host audio on stereo speakers, headphone surround is inactive with the reason `stereo_endpoint`, and the frame passes through as plain stereo.
- **Rendering:** each speaker channel is convolved with its HRIR pair. The FIR is direct-form, with a doubled history buffer so each dot product is contiguous and vectorizes.

  | Channels | Azimuth |
  |---|---|
  | FC | 0° |
  | FL / FR | ±30° |
  | SL / SR (7.1) | ±90° |
  | BL / BR (7.1) | ±135° |
  | Surrounds (5.1 source) | ±110° |

  LFE goes to both ears at -6 dB, unfiltered.
- **Level:** the sum is scaled so that uncorrelated equal-level noise on every speaker comes out within ±0.5 dB of the RMS of today's downmix. A safety soft clipper, the same as in B.3, runs last on what night mode didn't process, so no sample is clipped twice.
- **With night mode:** binaural rendering runs first, then night mode on the stereo result.
- **Live switching:**
  - If the stream started with 8-channel capture, switching between HRTF and the plain downmix happens live, with a crossfade over one packet.
  - If it didn't, turning it on reports `next_stream`: the sink format and the capture can't change without reopening.

**Inactive reasons**

| Reason | Meaning |
|---|---|
| `client_surround` | The client asked for 5.1 or 7.1 and renders its own speakers. |
| `game_only_audio` | Capture uses Game only, which is stereo. |
| `stereo_endpoint` | The PC's playback device is stereo. |
| `muted` | Audio isn't being streamed. |
| `next_stream` | It was turned on mid-stream without 8-channel capture. |

Each reason becomes a session warning (`s.launch.warnings`), a log line and a state in the native API.

**Caveats, in the console text**

- A game picks its audio format when it opens audio. A game that was already running stays stereo until it reopens its audio.
- A stereo-only game playing into the 7.1 sink comes out as speaker-virtualized stereo.
- The HRTF is a generic dummy head.

### B.5 HRTF data and licence

**Source**

- SADIE II, University of York, Zenodo record 10886409.
- Subject D1, the Neumann KU100. The generator confirms this from the SOFA metadata.
- 48 kHz HRIRs.
- Licence: Apache License 2.0, copyright 2018 University of York.
- Citation: Armstrong, Thresh, Murphy and Kearney, "A Perceptual Evaluation of Individual and Non-Individual HRTFs: A Case Study of the SADIE II Database", Applied Sciences 8(11):2029, 2018.
- Apache-2.0 data may ship inside a GPL-3.0 program.

**Generator: `rust/tools/hrir/generate.py`** (numpy, scipy, h5py)

1. Pin the SHA-256 of the input zip.
2. Pick the measured directions nearest to the ones in B.4.
3. Symmetrize mirror pairs.
4. Apply diffuse-field equalization: invert the average power spectrum over all directions, minimum phase, third-octave smoothed, limited to ±12 dB between 100 Hz and 16 kHz.
5. Trim the common onset. Truncate to 256 taps with a 32-sample half-Hann fade.
6. Assert that at least 99.5% of each filter's energy is kept, and that every pre-peak is 8 samples or less.
7. Write `rust/core/src/audio_dsp/hrir_sadie2_ku100_48k.bin`: little-endian f32, 9 directions × 2 ears × 256 taps, 18 KiB.
8. Write a `.json` manifest with the input URL and hash, the chosen directions, the processing steps and the output hash. A core test checks the output's SHA-256.

**Obligations**

- **Licence text:** copy the upstream `LICENSE.txt` verbatim to `rust/core/src/audio_dsp/LICENSE-SADIE-II.txt`.
- **THIRD_PARTY.md:** add a "SADIE II HRTF data" section to `rust/THIRD_PARTY.md` with the attribution, the citation and the full Apache-2.0 text. `build.ps1` already copies that file into `licenses\`.
- **NOTICE:** add an entry with the copyright, the citation and a statement of the changes made (Apache section 4(b)).
- **Upstream NOTICE:** if the zip contains a NOTICE file, add its text to NOTICE (Apache section 4(d)). Checked: `D1_HRIR_SOFA.zip` holds only the three SOFA files, and the record has no NOTICE file, so there is nothing to carry.
- **Mismatch:** if the downloaded `LICENSE.txt` isn't Apache-2.0, stop.

**Rejected sources**

| Source | Why not |
|---|---|
| CIPIC | Non-commercial terms. |
| MIT KEMAR (1994) | No licence is stated. |
| TH Köln KU100 sets | CC BY-SA 3.0, which isn't GPL-compatible. |
| Brown-Duda parametric model | Kept only as a fallback, built from code rather than data, in case SADIE II fails the licence check. |

### B.6 Settings, precedence, live control and the couch-menu hook

**Keys**

| Key | Default | Scope |
|---|---|---|
| `audio_night_mode` | off | Host. Overridable per device (`config_overrides`) and per app (`config-overrides`); added to `override_allowed`. |
| `audio_headphone_surround` | off | Same as `audio_night_mode`. |
| `sleep_after_idle_minutes` | 0 | Host-wide. |
| `sleep_while_suspended` | false | Host-wide. |

Validation: the audio keys and `sleep_while_suspended` are booleans. `sleep_after_idle_minutes` is 0, or 5 to 1440.

**Precedence, highest first**

1. A live change in this session.
2. The session choice: `audioNightMode=on|off` and `audioHeadphoneSurround=on|off` on `/launch` or `/resume`. They're read from `launch.options` like `hostDisplay`. Unknown values are logged and ignored.
3. The app.
4. The device.
5. The host.

**Live state**

- `Session` gains `audio: AudioControls`, a set of core atomics: night mode on, headphone surround requested, surround active plus a reason code, and a revision counter.
- The audio thread reads them once per packet with relaxed loads.
- The native API and the couch menu write them through `butterpollo_core::audio_dsp::{state, apply}`. **That pair is the hook for `docs/couch-menu-design.md`.**
- A live change applies to this session and is kept in memory per client (its paired uuid) and app: that client's next session of the app, a new launch or a resume, starts with it. A `/launch` or `/resume` parameter still wins. A host restart forgets it, and no config is written.

**Couch menu items** (`docs/couch-menu-design.md` section 4.3, `ITEMS` in `rust/host/src/couch_menu/items.rs`):

- `audio_night_mode`: Bool, Live, no host control. `value` and `set` call `audio_dsp::{state, apply}`.
- `audio_headphone_surround`: Bool, Live, no host control, with the same calls. When surround is inactive, `available` returns `Locked(reason)` with the B.4 reason. `next_stream` shows the "applies next stream" note.
- `sleep_after_idle_minutes`: Choice, needs host control, Live. Off, 15, 30, 60 and 120 are all valid under the rule above. `set` writes the host config through the same validated path the console's config update uses, and the 30-s tick picks it up. No sleep-specific code goes in the menu.
- Whichever of H3 and the couch menu's host brief lands second adds these three lines to `ITEMS`.

**Native API**

- `GET /api/native/v1/audio` returns `{ revision, settings: [Setting] }`.
  - It reuses `radeon::Setting`, with ids `audio.night_mode` and `audio.headphone_surround`, kind `toggle` and group `sound`.
  - Inactive reasons appear as a `reason` and `message` note on a setting that stays `state: configurable` with its `value`, since the owner can still write it.
  - With no session, the settings are `state: temporarily_unavailable` without a `value`, with reason `no_session`.
- `POST /api/native/v1/audio/settings` takes `{ "changes": [{ "id": ..., "value": ... }] }` and returns the new state.
  - It's for the owner of a streaming or suspended session, with View or Launch.
  - Host control isn't required, because the change affects only the caller's own stream.
  - It refuses with `no_session`, `not_owner` or `busy`, as the Radeon endpoints do.
- New features: `audio.night_mode` and `audio.headphone_surround`.

**Console**

- **Settings → Audio → Sound:** two new toggles. Headphone surround shows as inactive when Game only audio is selected.
- **Devices → device:** a new "Audio overrides" section with Host setting, On or Off.
- **Library → app:** a new `AudioSettings.svelte`.
- **Settings → General:** "Sleep after idle", and "Allow sleep while a game is left running".
- **Overview:** the stream card shows each stream's audio state.

### B.7 Nightjar

Everything is gated by native features, never by host brand.

- **Android:**
  - HostStreamSettings gets "Night mode" and "Headphone surround", each with Host setting, On and Off. They use new per-PC keys, added to `SettingsKeysTest`.
  - The launch query adds the parameters only when the host has the feature and the choice isn't Host setting.
  - The stream menu gets an audio panel next to the Radeon panel. It reads GET /audio when opened, writes on toggle, and shows "Applies next stream" for `next_stream`.
- **Android TV:** the same code, reachable with a controller (`NavigationDialogBuilder`, focus order).
- **Desktop:** `LaunchRequest` in nightjar-core gets the two choices. `session_ui.rs` gets an audio panel next to `radeon_panel`.
- **Dependency:** the `rubylight-native-api` git pin in `protocol/client/Cargo.toml` moves to the host commit that adds these types.

### B.8 Tests

**Night mode**

- Silence stays bit-exact zero.
- A -40 dBFS sine gets the leveller's full +12 dB, within 1 dB, and settles at about -31 dBFS RMS, under the target; a -20 dBFS sine settles to within 1 dB of the -23 dBFS target.
- A -6 dBFS sine gets the gain reduction the curve predicts, within 0.5 dB.
- A step from -40 to -6 dBFS never goes above 0.98 and settles within 10 ms.
- Gain is linked: a signal on FL alone leaves the other channels at zero.
- An impulse peaks at sample 0.
- Output is the same whether packets are 5, 10 or 20 ms or the whole signal is one block.
- The off ramp adds no step larger than the signal's own.
- NaN input is handled.

**Binaural**

- Output matches an f64 reference convolution in the test within 1e-5.
- An FL impulse reaches the left ear earlier and louder than the right.
- FL to (L, R) equals FR to (R, L).
- FC gives L = R.
- An LFE impulse gives L = R = 0.5 at sample 0.
- Noise energy is within 0.5 dB of the downmix.
- Output doesn't depend on packet size.
- The 5.1 and 7.1 direction sets are right.
- The data hash is right.

**Host**

- The precedence order is resolved correctly.
- Each inactive reason is set for its case.
- The silence packet is sized for the capture channel count.
- With both off, `Chain` passes the packet through.

**Bench and hardware**

- An `#[ignore]` release bench reports the time per packet, on this PC and on the laptop.
- End to end: play an 8-channel tone with `audio_probe` into the virtual sink and stream it to Nightjar in stereo. Each channel must land in the expected ear with the expected interaural level difference.
- The user listens on phone headphones.

## Risks

1. **Power requests may not show up.** `SystemExecutionState` might not reflect another process's `PowerSetRequest`. H1 proves it first. If it fails, sleep after idle waits for an architect decision.
2. **Some local use is invisible.** Input on the lock screen, and controller-only local play the host didn't launch that holds no execution state, aren't seen. Windows' own idle sleep has the same gap. Mitigations: the feature is off by default, and an app the host launched blocks sleep.
3. **The NIC fix drops the network.** Restarting the adapter cuts the network for a few seconds. The fix is refused while sessions exist, and the dialog says so. A console used over the LAN reconnects.
4. **Wake can break outside our control.**
   - Driver updates can reset NIC settings; the console check shows it.
   - Firmware settings can't be read.
   - Wake from shutdown and Fast Startup are labelled in the console.
   - Wi-Fi wake and Modern Standby wake are Experimental.
   - A VPN client can't wake a sleeping PC without a relay on its LAN; the `route` flag in `/wake` lets Nightjar say so.
5. **Wake grace may not be needed.** It may not reproduce here, and is then dropped. Nothing unmeasured ships.
6. **Headphone surround changes the sink.** It switches the sink to 7.1 for a stereo client.
   - Games that are already running stay stereo until they reopen audio.
   - Stereo games come out speaker-virtualized.
   - Concurrent streams on the same sink get today's downmix.
7. **Audio quality is judged by ear only.** A non-individual HRTF causes some front-back confusion and colouration. Night mode might pump on music. The user's listening check decides whether the defaults stay.
8. **CPU on small APUs.** The bench bound applies on the 780M as well. The lever is fewer taps (128). A partitioned FFT is not an option, because it adds latency.
9. **Licence text could differ.** The downloaded `LICENSE.txt` or a NOTICE file could differ from the record page. Check them before committing the data.
10. **Order of work.** The Nightjar pin can only move after H0 is pushed. Nightjar can only be tested end to end against a host build with H1, H2 and H3.

## Implementer briefs

General rules for every brief:

- Read the repo's `CLAUDE.md` first.
- Work in your own worktree under `C:\src`, with your own target directory.
- Never use `git stash`.
- Keep LF line endings and use the Edit tool.
- Write commit messages to a file (`git commit -F`), in the repo's commit style.
- Every fix gets a regression test.
- Failures are loud.
- Don't change `PROTOCOL_VERSION`.
- Before any install, stream, sleep or restart on this PC, `/serverinfo` must show RustHostSessionCount and RustHostPendingSessionCount at 0. Hardware steps also need the main session's go-ahead (the user is idle).
- Report: the commit SHAs, test output tails, measurements, and anything not verified.

### H0: contracts (host; run first, small)

**Files**

- `rust/core/native-api/src/lib.rs`: feature constants `WAKE = "wake"`, `AUDIO_NIGHT_MODE = "audio.night_mode"` and `AUDIO_HEADPHONE_SURROUND = "audio.headphone_surround"`, plus `pub mod audio; pub mod wake;`.
- `rust/core/native-api/src/audio.rs` (new): `Audio { revision, settings: Vec<radeon::Setting> }`, `AudioWrite { changes: Vec<radeon::Change> }`, the setting-id constants and the reason constants (`no_session`, `client_surround`, `game_only_audio`, `stereo_endpoint`, `muted`, `next_stream`).
- `rust/core/native-api/src/wake.rs` (new): `Wake`, `Target`, `Check`, `System` and `IdleSleep`, exactly as in A.6. Unknown enum values use `Lenient`.
- `rust/core/native-api/tests/wake_audio.rs` and `rust/core/native-api/tests/fixtures/{audio.json,audio_write.json,wake.json}`.
- `rust/core/src/config.rs`:
  - add `audio_night_mode` and `audio_headphone_surround` to `override_allowed`;
  - validate the changed values: the four keys of B.6, with minutes 0 or 5 to 1440;
  - add tests.
- `rust/core/src/lib.rs`: `pub mod wake;` and `pub mod audio_dsp;`.
- `rust/core/src/wake.rs` and `rust/core/src/audio_dsp/mod.rs` (new): stubs with a one-line doc comment only.
- `rust/windows/src/lib.rs` and `rust/windows/src/wake.rs`: the same kind of stub, `pub mod wake;`.
- `docs/native-api.md`: the `wake` and `audio` endpoint contracts, permissions, errors and features.

**Steps:** write the types and fixtures, the config changes with their tests, and the docs. No behaviour changes.

**Tests:** run `cargo +1.98.1-x86_64-pc-windows-msvc test -p butterpollo-core -p rubylight-native-api --locked --target-dir C:\src\rl-target`, then `C:\src\rl-quick.ps1 -Repo <worktree> -Crates butterpollo-core,butterpollo-windows -TargetDir C:\src\rl-target-h0`.

**Acceptance**

- The fixtures round-trip.
- An unknown field or enum value parses.
- `override_allowed("audio_night_mode")` is true and `override_allowed("sleep_after_idle_minutes")` is false.
- `sleep_after_idle_minutes=3` is rejected; 0, 5 and 1440 are accepted.
- `git diff` doesn't touch `PROTOCOL_VERSION`.
- Push to main; the main session gives the SHA to N1.

### H1: wake and sleep (host; after H0, parallel with H2)

**Files**

- `rust/core/src/wake.rs`: the MAC policy, the readiness judgement from the script's JSON, `IdleSleep` and `WakeGrace`, with tests.
- `rust/core/src/wake/fixtures/*.json` (new): anonymized MACs, locally administered `02:…`.
- `rust/windows/src/wake.rs`:
  - adapter facts: `GetAdaptersAddresses` plus `GetIfEntry2`;
  - the embedded `wake_nics.ps1` and its runner, for read and apply;
  - `GetPwrCapabilities`, `HiberbootEnabled`, `SystemExecutionState`, `PowerCreateRequest` and `SetSuspendState` with privilege enabling;
  - the `--idle-seconds` worker.
- `rust/windows/src/wake_nics.ps1` (new).
- `rust/windows/src/net.rs`: `local_mac` goes through the policy, with the same signature and output format.
- `rust/windows/Cargo.toml`: only if a `windows` feature is missing.
- `rust/host/src/wake.rs` (new):
  - a 30-s tick against a `PowerSystem` trait;
  - status;
  - wake grace;
  - `pub fn status(h) -> butterpollo_core::wake::Status`;
  - `pub fn readiness(h, refresh: bool) -> rubylight_native_api::wake::Wake`;
  - `pub fn request_fix(h, FixPlan) -> Result<(), FixRefusal>`;
  - `pub fn note_console_activity(h)`;
  - `pub fn note_client_request(h, paired: bool)`.
- `rust/host/src/main.rs`:
  - `mod wake;`;
  - Args `--idle-seconds` and `--apply-wake-settings [--fast-startup-off]`;
  - a `wake` field in the `--diagnostics` JSON.
- `rust/host/src/runtime.rs`: the 30-s cadence, through `spawn_exclusive`.
- `rust/setup/src/wake.rs` (new): the remembered choice (the `usbip.rs` pattern) and the call to the installed exe.
- `rust/setup/src/main.rs`: the optional step and the `--wake-on-lan` flag.

**Steps**

1. **Prove the power-request detection first.** Write the `#[ignore]` test: a child process holds `PowerSetRequest(PowerRequestSystemRequired)`, and `SystemExecutionState` must show it. Run it as SYSTEM with a one-shot `schtasks /RU SYSTEM` task, as `rust/release/elevated.ps1` does. If it fails, stop and report; don't build A.5 on it.
2. Write the MAC policy and its table tests, then switch `local_mac` to it.
3. Write the facts reader and `wake_nics.ps1 read`. Record this PC's JSON as SYSTEM and anonymize the MACs for the fixtures.
4. Write the readiness judgement, `apply`, `--apply-wake-settings` and the setup step.
5. Write `IdleSleep` and the host tick, then `WakeGrace`. Call `note_client_request` from where nvhttp handles HTTPS requests, as a single line through `crate::wake`. That's the only nvhttp edit.

**Tests**

- `cargo +1.98.1-x86_64-pc-windows-msvc test -p butterpollo-core --locked --target-dir C:\src\rl-target wake`
- `C:\src\rl-quick.ps1 -Repo <worktree> -Crates butterpollo-core,butterpollo-windows,butterpollo,butterpollo-setup -TargetDir C:\src\rl-target-h1`

**Acceptance**

- The policy tests pass. This PC's fixture gives the Realtek MAC for an Ethernet route, a vEthernet route and a Tailscale route.
- serverinfo `<mac>` on this PC is unchanged in value and format.
- `butterpollo.exe --diagnostics` run as SYSTEM shows the Realtek adapter ready, S3 true, Modern Standby false and Fast Startup inactive.
- The step 1 test passes.
- With the default config, the tick never calls `sleep()`. The fake-`PowerSystem` tests cover every blocking condition.
- `--apply-wake-settings` against a fake runner produces the exact keyword and WMI plan.
- Don't run the real apply on this PC; its settings are already correct.
- Hardware (A.8) only with the go-ahead. Report the measured time from wake to serverinfo.

### H2: audio DSP (host; after H0, parallel with H1)

**Files**

- `rust/core/src/audio_dsp/mod.rs`: `Chain`, `Choice` resolution and `Controls`.
- `rust/core/src/audio_dsp/night.rs`, `binaural.rs` and `hrir.rs` (new).
- `rust/core/src/audio_dsp/hrir_sadie2_ku100_48k.bin` and `.json` (new, generated).
- `rust/core/src/audio_dsp/LICENSE-SADIE-II.txt` (new).
- `rust/core/src/session.rs`: `AudioControls` on `Session`.
- `rust/core/src/audio.rs`: only if a layout helper is needed.
- `rust/windows/src/audio.rs`: a source-layout accessor on `Loopback`.
- `rust/host/src/stream.rs`: inside `fn audio` only. Capture channels, the route and loopback channel count, silence sizing, `Chain`, and session warnings for inactive reasons.
- `rust/host/src/stream/audio_sink.rs`: only if `collect` must hand out mutable packets.
- `rust/tools/hrir/generate.py` (new).
- `rust/THIRD_PARTY.md` and `NOTICE`.

**Steps**

1. Download `D1_HRIR_SOFA.zip` and `LICENSE.txt` from Zenodo record 10886409 into a new, empty directory under the scratchpad. Check the licence (B.5). Run the generator with `python -I`. Commit only the outputs and the licence copy.
2. Write night mode with golden tests (B.8).
3. Write the binaural renderer with an f64 reference test.
4. Wire `Chain` into `fn audio`. Resolve the choice: session option, then app, then device, then host, with the live atomics on top. Add the inactive reasons.
5. Add the bench.

**Tests**

- `cargo +1.98.1-x86_64-pc-windows-msvc test -p butterpollo-core --locked --target-dir C:\src\rl-target audio_dsp`
- Bench: `cargo +1.98.1-x86_64-pc-windows-msvc test -p butterpollo-core --release --locked --target-dir C:\src\rl-target audio_dsp -- --ignored --nocapture bench`. Run it on the 780M laptop too, over PowerShell ssh.
- `C:\src\rl-quick.ps1 -Repo <worktree> -Crates butterpollo-core,butterpollo-windows,butterpollo -TargetDir C:\src\rl-target-h2`

**Acceptance**

- All B.8 tests pass.
- The bench is under 0.25 ms per 5 ms packet on both machines. Report the numbers for `rust/PERFORMANCE.md`.
- With both settings off, the output is bit-identical to today's.
- The generator re-run reproduces the `.bin` SHA-256.
- A stereo stream with headphone surround on logs "headphone surround active (8 to 2)" and still uses the stereo Opus layout.
- Each inactive reason produces its session warning.
- The end-to-end tone check (B.8) only with the go-ahead. The user's listening check is reported as pending, if it hasn't been done.

### H3: host API, console and docs (after H1 and H2)

**Files**

- `rust/host/src/native_api.rs`: routes, and the features list adds `wake` and both audio features.
- `rust/host/src/native_api/audio.rs` (new): `state`, `apply`, read and write handlers, using the owner check from `radeon.rs`.
- `rust/host/src/native_api/wake.rs` (new).
- `rust/host/src/web.rs`: routes `GET /api/power` and `POST /api/power/wake/fix`. Its auth middleware calls `crate::wake::note_console_activity`.
- `rust/host/src/web/power.rs` (new).
- `rust/host/src/web/config.rs`: metadata, if the readiness check needs it.
- `rust/web/src/lib/schema/basics.ts`, `rust/web/src/lib/host-readiness.ts` (plus its `.test.mjs`) and `rust/web/src/lib/api.ts`.
- `rust/web/src/components/overview/HostReadiness.svelte` and `StreamCard.svelte`.
- `rust/web/src/components/devices/DeviceEditor.svelte` and `device.ts`.
- `rust/web/src/components/library/AudioSettings.svelte` (new), `AppEditor.svelte` and `draft.svelte.ts`.
- Wherever the stream status JSON that `StreamCard` reads is built: find it from `api.ts` and add an `audio` object.
- `docs/configuration.md`, `docs/api.md`, `docs/privacy.md`, `docs/troubleshooting.md` (a Wake-on-LAN section) and `docs/features.md`.
- The `docs/rubylight-3.0-plan.md` status table.
- `rust/RELEASE_NOTES.md`, under `## Unreleased`.
- `rust/host/src/couch_menu/items.rs`: only if it's already on main. Add the three B.6 entries. Otherwise leave them to the couch menu brief.

**Steps:** the native API endpoints and tests, then the console endpoints (409 while sessions exist; 202, then apply), then the web UI, then the docs.

**Tests**

- `C:\src\rl-quick.ps1 -Repo <worktree> -Crates butterpollo -TargetDir C:\src\rl-target-h3`
- `cd rust\web; npm ci --no-audit --no-fund; npm run check; node --test "*.test.mjs"; npm run build`. Kill any stray node or vite first.

**Acceptance**

- **Native API permissions:**
  - unpaired: refused;
  - paired with no session: `no_session`;
  - the owner's write takes effect on the next packet, raises the revision, and GET reflects it;
  - a non-owner gets `not_owner`;
  - the features list includes the three new features;
  - `/wake` matches the A.6 fixture shape.
- **Console fix:** returns 409 with an active session, and the dialog lists the exact changes.
- **Look at the result:** screenshot an isolated test host (`--config-dir C:\tests\<p> --port 48123 --bind 127.0.0.1`), showing Overview readiness, the Settings audio and power rows, the device overrides and the app audio overrides.
- Run the full gate once for the H0 to H3 batch, in the background. Then push.

### N1a: Nightjar wake (after H0 is pushed; before N1b)

**Files**

- Android:
  - `app/src/main/java/com/limelight/nvstream/wol/WakeOnLanSender.java` and its test.
  - `app/src/main/java/com/limelight/nvstream/wol/WakeTargets.java` (new) and its test.
  - `app/src/main/java/com/limelight/nativeapi/NativeCapabilities.java`, `NativeApiClient.java` (`wake()`) and `NativeWake.java` (new).
  - The wake call sites: `app/src/main/java/com/limelight/computers/HostActions.java`, `ShortcutTrampoline.java`, `console/ConsoleHomeActivity.java`, `console/HostWaker.java` and `console/HomeWake.java`.
  - The wake hunk only (around line 441 to 460) of `preferences/HostStreamSettings.java`.
- Rust:
  - `protocol/client/src/http/native.rs` (`wake()`) and `protocol/client/Cargo.toml` (the pin).
  - `protocol/android/src/native_api.rs` (op `wake`).
  - `protocol/desktop/src/wol.rs`, `hosts.rs` and `main.rs`.
  - `app/src/main/res/values/strings.xml`.

**Steps**

1. Bump the pin to H0's SHA.
2. Add `wake()` to the native clients.
3. Add the `WakeTargets` store.
4. Change the sender to use every MAC, port 7, subnet broadcasts and `ff02::1`, with the signature `sendWolPacket(Context, ComputerDetails)`. Remove the old signature, so the compiler finds every caller.
5. Move every caller onto `HostWaker`.
6. Add the desktop `WakeWait` state machine and the "Waking <PC>…" UI.

**Tests**

- `.\gradlew.bat :app:testDebugUnitTest --tests "com.limelight.nvstream.wol.*" --tests "com.limelight.console.*Wake*" --offline`
- In `C:\src\rubylight-android\protocol`, first `$env:PATH = "C:\msys64\ucrt64\bin;$env:USERPROFILE\.cargo\bin;" + $env:PATH`, then:
  - `cargo test -p nightjar-core native`
  - `cargo test -p nightjar-android native_api`
  - `cargo test -p nightjar-desktop wol`

**Acceptance**

- For two MACs, the packet plan covers:
  - 255.255.255.255;
  - each interface's broadcast;
  - the host's `/wake` broadcasts;
  - ports 7, 9, 47009 and the dynamic set;
  - one payload per MAC;
  - `ff02::1`, whose send errors don't fail the wake.
- A host without `wake` gets the serverinfo MAC only.
- The desktop `WakeWait` tests mirror `WakeLaunchTest`.
- Manual tests with a sleeping PC are run by the user (or by the NUC, for the packet itself) and are reported as pending otherwise.

### N1b: Nightjar audio (after N1a and after H3 is on a host build)

**Files**

- Android:
  - `app/src/main/java/com/limelight/nativeapi/NativeAudio.java` (new), plus the audio methods in `NativeApiClient.java`.
  - `app/src/main/java/com/limelight/console/NativeAudioController.java` (new) and `ui/AudioPanelView.java` (new), with the stream-menu hook where `RadeonPanelView` is mounted.
  - The audio prefs hunk of `preferences/HostStreamSettings.java` and its preference XML.
  - `PreferenceConfiguration.java`, `nvstream/ConnectionContext.java`, `nvstream/http/NvHTTP.java` (launch query) and `Game.java`.
  - `SettingsKeysTest`.
  - `strings.xml`.
- Rust:
  - `protocol/client/src/http/native.rs` (`audio`, `write_audio`) and `protocol/client/src/http/launch.rs` (`LaunchRequest` audio choices).
  - `protocol/android/src/native_api.rs` (ops `audio` and `audio_write`).
  - `protocol/desktop/src/session_ui.rs` and `protocol/desktop/src/audio_panel.rs` (new).

**Steps:** the native calls, then the launch parameters gated by features, then the per-PC preferences, then the stream-menu panels on Android and desktop. The panels follow `NativeMenuController`'s threading rules: never call from the render, decode or input threads.

**Tests**

- `.\gradlew.bat :app:testDebugUnitTest --tests "*SettingsKeysTest" --tests "*NativeAudio*" --tests "*NvHTTP*" --offline`
- In `protocol`: `cargo test -p nightjar-core launch native` and `cargo test -p nightjar-android native_api`.

**Acceptance**

- The launch query carries the parameters only with the features present and a choice other than Host setting.
- Panels are hidden on hosts without the features.
- `next_stream` shows "Applies next stream".
- `SettingsKeysTest` gains the new keys and no existing key changes.
- Desktop builds.
- The phone run is done by the user. Read the host log afterwards for the audio-state lines.
