# Nightjar device checklist

[Documentation](../docs/index.md) · [Host feature matrix](../docs/PARITY-MATRIX.md)

Record the app build, host version, phone/tablet/TV model, SoC, Android build, controller model/firmware, display mode and network for each run. Attach the relevant copied stats, CSV, host logs and captures. Unchecked items below are a test plan, not claimed results.

## Pairing, identity and host actions

- [ ] Discover and manually add IPv4/IPv6 hosts, including custom ports and multiple network interfaces. Verify offline, unreachable, wrong-address and cancellation states.
- [ ] Pair by displayed PIN and host-generated one-time PIN; exercise wrong/expired PIN, bad passphrase, cancellation, backgrounding and retry.
- [ ] Update in place with the same key; clear data/restore onto another device and confirm fresh pairing. Revoke the old identity on the host.
- [ ] Test a disabled device and each independent permission. Revoke access during a session; verify listing, viewing, launching, all input types, clipboard directions and commands remain host-controlled.
- [ ] Refresh app UUIDs, names, numeric IDs, host ordering and versioned covers. Exercise hidden apps, pinned shortcuts and changed/removed library entries.
- [ ] Confirm Disconnect preserves the app and reconnect resumes it. Test Quit app, replacement and Terminate with Cancel, Back, Home, rotation, expiry and explicit confirmation.
- [ ] With two paired clients, test ownership changes, Remote Monitor and Input-only. Monitor must not send input; Input-only must not time out for absent video or enter PiP. End only the intended remote role.
- [ ] Transfer empty, Unicode and oversized clipboard text with send/read grants varied independently. Exercise missing/reordered commands and host app restrictions; sending is not an execution acknowledgement.
- [ ] Pair while awake, then test Wake-on-LAN from sleep with Ethernet, Wi-Fi, VPN, multiple subnets and a broken route. Record actual wake, not merely packet submission.

## Video, pacing and diagnostics

- [ ] Compare H.264, HEVC and AV1 at 720p, 1080p and native resolution; 60/120 FPS and higher where supported. Record decoder name, profile, accepted low-latency keys and all stage timings.
- [ ] Compare all four pacing modes at matched and mixed panel rates, including 60→90, 60→120, 90→120 and fractional rates. Check first-frame output, bounded queues, starvation, static-to-motion transitions and recovery.
- [ ] Compare received, rendered (submitted) and on-screen (callback-observed) FPS. Check delayed/missing render callbacks, estimated drops after five seconds and unavailable fields. Use a trace/camera to measure physical presentation separately.
- [ ] Enable VRR on Android 15, Android 16 with and without confirmed adaptive refresh, and an older fixed-rate device. Vary cadence through 30/59.94/60/90/119.88/120 FPS, idle, resume and surface recreation.
- [ ] Compare SurfaceView and compatibility TextureView where allowed. Record physical panel Hz, app render-rate override, cadence hints, battery saver and thermal restrictions.
- [ ] Test HDR10 PQ highlights, gradients, black/white patches, metadata changes, HDR↔SDR transitions and 10-bit SDR. Confirm no stale metadata, clipping or eight-bit intermediate output.
- [ ] Compare 4:2:0/4:4:4 and limited/full range with colored single-pixel text. Verify negotiated fallback and HDR priority.
- [ ] Test Statistics/Debug switching from the stream menu, keyboard toggle, persistence, Copy stats, missing values, health thresholds and PiP hiding. Long-press switching belongs only to the debug overlay preview. Compare exported CSV columns with the displayed stages.
- [ ] Compare Phone performance hints on/off, decoder hints and sustained thermal load for at least 20 minutes. Keep network loss separate from decoder/presentation stalls.

## Upscaling

- [ ] Compare Off, Bilinear, FSR and SGSR at 720p/1080p → native, with 0/50/100% sharpening. Check text, diagonals, texture, motion, ringing, color, crop, portrait and stretched output.
- [ ] Confirm native/larger input, HDR and 10-bit use direct output with the appropriate reason. For lower-resolution SDR PyroWave, verify SGSR is used for both the FSR and SGSR selections and that Debug reports the active path. Check missing GLES/OES support on the conventional path.
- [ ] Compare added-time estimates, latency traces and battery use at 60/120 FPS. Do not count decoder texture callbacks as displayed frames.
- [ ] Overload the GPU and trigger repeated drops, rotation/resize and renderer recovery. Confirm one direct-output fallback, released resources and no reactivation until reconnect.
- [ ] Save/reset per-PC overrides, exercise old-profile inheritance and both presets, and test sharpening cancellation and persistence.

## Network and PyroWave

- [ ] Run the ten-probe connection test; distinguish HTTP probe failures from UDP video loss. Cancel the optional 32 MiB download and verify the host-link and throughput units.
- [ ] Inject sustained and bursty loss, jitter and reordering separately from decode overload. Test manual bitrate, both automatic controllers, host caps, missing metrics, permission denial and reconnect.
- [ ] On supported 64-bit Vulkan devices, start PyroWave at 720p/60 on a fast wired link, then compare Wi-Fi and higher geometry/rates. Record readiness, present mode, host bitstream compatibility and actual throughput.
- [ ] On a compatible record-framed host, confirm surviving regions update, lost regions recover and reconnect clears old imagery. Lose first/middle/last datagrams and a burst spanning recovery blocks; a datagram can affect several records.
- [ ] Check ordinary framing for unknown host versions and conventional-codec fallback for unsupported Vulkan features, surfaces or incompatible bitstreams. Test 8/10-bit and 4:2:0/4:4:4 combinations.
- [ ] Verify PyroWave completed decode, last GPU decode, queue delay and missing-record metrics separately. Test bitrate reduction under load, slow recovery after a healthy period, and the 500 Mbps runtime cap.

## Controllers, touch and keyboard

- [ ] Test USB/Bluetooth controllers, multiple players, digital remaps, face swap, stick drift, detach/reconnect and focus loss. Include two buttons mapped to one output and a disabled button.
- [ ] With USB DualSense disabled, confirm ordinary Android input and no direct-driver permission request. Enable it, cancel the explanation, then retry; deny and subsequently grant Android USB permission.
- [ ] On DualSense/Edge, verify one player slot, sticks, hat/buttons, analog triggers, both touch contacts, gyro/accelerometer, battery, LEDs, lightbar and each rumble motor.
- [ ] Exercise host-driven left/right/both/off trigger effects and interleaved lightbar/rumble updates. Test Edge paddles independently from Fn buttons, multiple pads, unplugging during feedback and reconnect.
- [ ] Confirm rumble/trigger reset on exit, no stuck inputs or duplicate slots, and USB audio interfaces remain available. Retain real USB captures when validating synthetic fixtures.
- [ ] Compare controller/device motion fallback and vibration strength. Test Start-held mouse mode, stick scrolling and touchpad mouse behavior.
- [ ] Test trackpad clicks, drag/scroll, direct mouse and native multi-touch/pen; change modes during gestures and test Android edge cancellation, focus loss and keyboard opening.
- [ ] Verify physical mouse distance and input cadence at fractional/high panel rates, capture/release, wheel and Back/Forward buttons, short clicks and immediate-input mode.
- [ ] Test keyboard layouts, AltGr, numpad Enter/separator, Print Screen, Unicode text, all documented shortcuts and key release across dialogs/focus loss.
- [ ] Move, resize, save and reset the on-screen gamepad; test opacity, vibration, Guide and L3/R3-only mode.

## Audio and lifecycle

- [ ] Compare stereo/5.1/7.1, effects, matching/mismatched output rates and output-route changes. Record AAudio/AudioTrack selection, actual sharing/performance mode, underruns, recovery and A/V sync.
- [ ] Exercise rapid start/stop/reconnect, app switching, network loss, screen lock, rotation, surface recreation, background/foreground and process recreation. Check stale dialogs, orphan decoders and duplicate sessions.
- [ ] Test PiP at 16:9, portrait and ultrawide ratios: enter, expand, close, disconnect, reconnect and stale actions. The host game must remain running.
- [ ] Run the 10-second local benchmark offline, with no input, with different input devices and under load. Missing metrics must stay unavailable; verify reset/cancel/lifecycle behavior.
- [ ] Export problem reports and CSV to real receiving apps; inspect redaction, decoder configuration, last-crash handling and cancellation of sharing.

## Interface, packaging and launchers

- [ ] Check 360 × 800 dp and 393 × 851 dp, portrait/landscape, font scales 1.0/1.3/2.0, gesture/three-button navigation, English/German and long host/app names.
- [ ] Visit library/roles, all five settings sections, search/empty results, profiles, help and every stream subdialog. Check wrapping, scrolling, IME insets, Back behavior and reachable Save/Cancel controls.
- [ ] With TalkBack and keyboard/D-pad, check labels, headings, focus return and touch targets. Search titles/categories/accents/punctuation and verify original dependencies after opening a result.
- [ ] Export ES-DE configuration and covers; launch from ES-DE, Daijisho and Pegasus. Test renamed games with stable IDs/UUIDs, interrupted writes and retry, custom-cover preservation, unreadable/malformed files, missing PCs/apps, URI grants and return to the frontend.
- [ ] Test debug and R8 release artifacts and the production signing identity. Retain GPL and bundled shader/native notices.
- [ ] Verify packaged native ELF and ZIP alignment and run on a 16 KB page-size Android device. Check all included ABIs and the 64-bit-only PyroWave path.
- [ ] Capture current Nightjar setup, settings, PC profile and live stream-menu/overlay screens. UI fixture captures do not establish a live host or hardware decode result.
