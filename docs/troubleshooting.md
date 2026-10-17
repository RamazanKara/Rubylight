# Troubleshooting

[Documentation](README.md) · [Getting started](getting-started.md) · [Configuration](configuration.md)

Start with the current stream's settings, the console and the logs. Record the time of a failure before changing anything, and change one setting at a time. Capture and performance probes add work to the GPU; use an idle session for those comparisons.

**Find a symptom:** [Pairing](#moonlight-cannot-find-or-pair-with-the-pc) · [Console or port](#the-console-will-not-open-or-a-port-is-occupied) · [Black screen](#black-picture-no-display-or-wgc-fails) · [Audio](#audio-cuts-out-or-lags) · [Blurred PyroWave](#pyrowave-shows-blurred-grey-blocks) · [HDR colour](#hdr-looks-washed-out-too-bright-or-different-between-clients) · [Stutter](#low-latency-is-reported-but-motion-still-stutters) · [Playnite](#playnite-does-not-launch) · [RTSS](#rtss-does-not-start-or-the-game-ignores-the-cap) · [Display restoration](#monitors-stay-on-or-the-display-layout-does-not-return) · [Controllers](#a-controller-or-other-input-does-nothing) · [Steam Deck](#a-steam-deck-has-no-gyro-trackpads-or-back-grips) · [Updates](#an-update-stays-queued) · [Logs and support](#logs-and-a-useful-report)

## What the stream card warnings mean

The stream card shows the active encoder and warnings for the current session. Each warning also writes a `WARN` log with its cause and a next step. Capture recovery and audio capture warnings clear when those paths recover. Dropped packets, frames and input, and audio device changes, stay on the card for 30 seconds after they last happened. A warning describes a fallback or limitation, not a measured latency penalty.

- **Encoder fallback, software or CPU frame copies:** if AMF (AMD) or NVENC (NVIDIA) cannot start, Automatic uses another hardware encoder when one works, such as Quick Sync on an Intel integrated GPU, and says so on the card. It never falls back to software: with no working hardware encoder the stream fails with the AMF/NVENC error in the log. Check that error and the graphics driver. Software must be selected explicitly, preferably at a lower resolution and frame rate; CPU frame copies and software encoding can limit fps.
- **Graphics queue conversion/copies:** compute setup failed. A busy game can delay capture or conversion. Lower game GPU load or check the AMD driver.
- **Desktop Duplication / capture interrupted:** WGC or its helper could not capture. UAC and lock screens can cause a temporary change. Unlock the desktop; if the warning persists, check the helper error. A display size change causes capture to reopen.
- **Physical display, refresh, HDR or limiter:** the requested setup is unavailable, unsupported or owned by another stream. Check the actual display mode and RTSS. A 60 Hz source cannot supply 116 fresh pictures per second.
- **Audio:** a missing virtual sink can send sound to the host's speakers. Device changes and capture failures can pause sound. The host-loss warning names what was lost: sound Windows dropped because the host's audio sender read too late (a busy CPU or a host problem, not bitrate), or audio packets the network adapter refused (lower the video bitrate or check the adapter).
- **Bitrate, pacing or FEC:** the encoder/wire budget was limited, packets were dropped, or large frames lost FEC protection. Check Maximum bitrate and the client bitrate. Runtime bitrate updates have a 500 Mbps cap. Leave headroom for audio, packet headers and FEC.
- **AMF rate-control limit rejected:** the driver accepted the target bitrate but refused a peak-rate, buffer or frame-size limit. Later bitrate changes retry using that limit's last accepted scaling basis; an unreadable property does not clear its warning. If it persists, reconnect at the desired bitrate or update the AMD driver.
- **Input:** the selected driver or Windows input API is unavailable. Video continues; repair the named driver or choose a supported profile.
- **PyroWave:** the client negotiated another codec, or bitrate/recovery protection is insufficient. Use a PyroWave-capable client and a fast wired network, or choose HEVC/AV1.


## Moonlight cannot find or pair with the PC

- Open **https://localhost:47990** on the host. If it does not open, check the host/service first.
- Confirm both devices are on the same local network, then try adding the host's local IP address in Moonlight. Guest networks or access-point isolation can prevent devices from reaching each other.
- Check **Settings → General → Allow pairing**. Enter the four-digit PIN **shown by Moonlight** into the matching pending request on **Devices**.
- If discovery alone fails, check **Settings → Network** for the discovery and bind-address settings. An address restricted to `127.0.0.1` is reachable only from the host itself.
- If setup reported a firewall-rule failure, read the setup log for the exact error and check Windows Firewall's allowance for the installed `butterpollo.exe` on your local network. Avoid disabling the entire firewall to diagnose one application.

If the device pairs but cannot launch Desktop, open **Devices → Edit** and check its enabled state and **List apps**, **View streams** and **Launch apps** permissions. A device missing mouse or keyboard permission can receive video without that input working.

## The console will not open, or a port is occupied

The default base port is `47989`; the console uses `47990`. A changed **Base port** moves the console to the next port up. Check the active profile's `sunshine.conf` if the launcher reports another address.

**Start Rubylight.exe** reopens the correct console when that profile is already running. If it reports another streaming host on the port, close the conflicting Sunshine, Apollo, Vibepollo or second Rubylight instance, or assign [separate profiles and ports](configuration.md#multiple-instances). Restarting another copy on the same port does not solve the conflict.

For an installed host, check **Rubylight** in Windows Services. Its internal service name is `ApolloService`. If it is stopped, read `service.log` before starting it again. The [log locations below](#logs-and-a-useful-report) distinguish service and portable profiles.

## Black picture, no display, or WGC fails

1. On **Overview → Host readiness**, inspect **Screen capture**, **Video encoder** and **Virtual display**. A physical monitor must be active, or the configured virtual display must be available.
2. Check **Settings → Display → Display** and any app/device override. Confirm that the selected display is the one containing the desktop or game.
3. For service-mode WGC, keep a Windows user signed in. Rubylight starts its WGC capture worker in that user's session. A locked or UAC desktop uses the Desktop Duplication recovery path; WGC is retried when the normal desktop returns. A stream started while Windows is locked sets up its display on the lock screen; if the virtual display cannot be set up there, the stream shows the physical display and the stream card says so. Start the stream again after signing in.
4. Read the `capture backend opened` log entry for the backend that actually opened. `requested_capture=wgc` describes the request and can appear even when capture falls back.
5. If the virtual-display status reports access denied, use the installed Windows service and check that the bundled driver is ready. A portable host having administrator rights is not equivalent to the driver's service access.

For a controlled comparison, disconnect first, set **Settings → Video → Capture method** to **Desktop Duplication**, reconnect at the same resolution/rate, and record whether the symptom changes. Return to **Automatic** afterward unless the explicit choice is needed. Keep the original failure log so a successful fallback does not hide the WGC error.

The installed service always runs the host in the session attached to the physical console. When Remote Desktop takes over a signed-in desktop, Windows gives the console a new session at the sign-in screen, and the host moves there. Closing the Remote Desktop window leaves that desktop disconnected. A Moonlight stream then shows the console's sign-in screen. Signing in there as the same user makes Windows reconnect the disconnected desktop to the console and end the sign-in session, which also ends that stream. Reconnect Moonlight once the desktop is back; the host restarts in that session within a few seconds. The host logs a warning naming the disconnected session when a stream starts in this state.

For a host started inside the Remote Desktop session (not the installed service), open **Maintenance → Windows session → Return to console**. With the installed service the host runs on the console, so this action is unavailable and its status explains why. This explicit action returns only the Windows session running this host to the physical console. Its desktop becomes visible at the PC and an active RDP connection disconnects. Disconnect Moonlight streams first. The console must have no signed-in user, or contain a disconnected session whose Windows user SID matches the host session's user. Active sessions, different users and unverified identities remain protected. The host may restart while Windows switches sessions. Refresh the status once it returns, then reconnect Moonlight. If Windows cannot verify both users or refuses the operation, use the installed service or sign in at the PC; no Windows password is collected by this action.

Launch requests check the host's own Windows session before and after preparation, and again before RTSP playback. A disconnect during display or app preparation therefore returns a retry error instead of accepting that pending stream. The log records the session ID, console ID, connection state, protocol and acceptance reason so a Windows classification problem can be distinguished from a transition during preparation; a refused check is logged as a warning. Physical console sign-in and lock-screen support remain unchanged. These checks cannot prevent Windows from switching sessions after playback has started.

## HDR looks washed out, too bright, or different between clients

Separate the host's HDR source, the negotiated stream and the client's display output:

- Check **Windows Settings → System → Display** for the **selected streamed display**. HDR on another monitor does not establish HDR on this one; wide-gamut SDR is also different from HDR.
- Check the client's HDR option and the Overview stream card's **HDR** badge. In the host log, `hdr=true`, the codec and `source_pixel=RgbaF16` identify a negotiated HDR stream with floating-point capture. FP16 capture alone does not prove that the game is producing HDR highlights.
- Start with **Settings → HDR → Display HDR: Match the stream** and **HDR request: Follow the device**. A display-only override does not change what the client negotiated. Check app/device overrides too.
- Confirm the game's HDR setting and the receiving display's HDR mode. For a comparison between two clients, keep the codec, scene and host display the same; AV1 HDR on one device versus HEVC SDR on another is not an equivalent check.
- Report whether black levels, mid-grey menus, bright highlights or colour saturation are wrong. Include the client app/version, device and display model, codec, stream resolution/rate and the host log time.

Version 2.2.16 and earlier can retain stale display colour information after a mode change. The [2.2.17 HDR refresh fix](../rust/RELEASE_NOTES.md#new-in-2217) updates that information and reopens capture when its colour space changes. On older versions, if HDR was enabled after the stream started, reconnect once with HDR already enabled on the selected host display and compare the same scene. Include the log from stream startup, especially `stream configured`, `source_pixel`, `hdr`, `full_range`, and any AMF range or HDR-metadata warnings; a timing-only excerpt cannot establish the negotiated colour path.

[2.2.18](../rust/RELEASE_NOTES.md#new-in-2218) also fixes missing or incorrectly scaled HDR metadata on native and FFmpeg NVENC paths, delivers changed metadata with a keyframe, and retries transient AMD metadata failures. An `AMF output range mismatch` or `AMF full-range output cannot be established` error now prevents an encoder from silently producing the wrong range. If no configured fallback works, reconnect with limited range, choose another supported encoder or update the AMD driver. Optional input-range properties rejected by older drivers remain nonfatal.

If HDR looks washed out with no HDR badge on the TV, update the host: since 2.0.1 every client gets the HDR state once per stream ([#11](https://github.com/RamazanKara/Rubylight/issues/11)). Rubylight's native HEVC and AV1 HDR path is checked down to reference pixels; see the [native HDR record](../rust/PERFORMANCE.md#final-native-virtual-hdr-pixels-excluding-physical-panel-calibration).

## Audio cuts out or lags

Audio and video share the network path. On Wi-Fi, a video burst can fill the adapter or access point's queue even when the average bitrate looks reasonable. Lost HEVC or AV1 frames can then require a larger recovery frame.

1. Lower the client's bitrate and compare the same scene. On AMD, HEVC or AV1 is a better starting point for Wi-Fi than PyroWave. Compare a wired connection when possible.
2. A fast host Ethernet connection does not prove that the client or the rest of the path can carry equally fast bursts. For a known constrained path, compare a fixed `pacing_max_bitrate_kbps` below its capacity but above the stream's wire demand, including FEC. For example, `80000` is a reasonable comparison for a 40 Mbps stream with 20% FEC on a 100 Mbps path. This adds send time without lowering picture bitrate. Save and reconnect, then compare the same scene and loss statistics. Explicit limits retain a 110% encoder-bitrate floor; they are not automatically raised for FEC.
   When keyframes themselves are lost on such a path, the picture freezes on every recovery. The host then paces the rest of the stream at twice the bitrate, as on Wi-Fi, and the stream card says so (**Slow down after lost keyframes**, on by default). A fixed limit avoids the first frozen moments too.
3. **Adaptive packet pacing (experimental)** is an optional alternative when the suitable rate is unknown or manual tuning is inconvenient. Leave the pacing limit at `0` and enable `host_adaptive_pacing`. It starts conservatively, then probes faster rates with active video and healthy control feedback; a validated loss during a probe restores the preceding rate. Startup is slower and probing can still lose packets. It is off by default and does not apply with a positive fixed limit, Host adaptive bitrate or PyroWave. If loss continues, lower the client's bitrate too; pacing cannot create network capacity.

Compare Moonlight's loss statistics and the host log before and after. `reference_invalidations` counts reference-loss feedback separately from `idr_requests`; neither is a count of lost packets. PyroWave has no inter-frame references, so its recovery feedback does not force another encode or increase FEC. A steadily rising request counter alone does not establish Wi-Fi loss. [Pacing details](configuration.md#capture-and-video) and [transport evidence](../rust/PERFORMANCE.md#october-7-pyrowave-recovery-feedback) describe the limits of the local tests.

## PyroWave shows blurred grey blocks

A very low bitrate can leave PyroWave with only coarse brightness and little colour or fine detail. The old one-bit-per-pixel warning was too low for the measured desktop pictures. The stream card and log now distinguish `PyroWave bitrate is too low` (red, below the severe-loss floor) from `PyroWave bitrate is below recommended` (amber, text and textures may lose detail).

At 60 fps the floor/recommendation are **139/277 Mbps for 720p**, **187/399 Mbps for 1080p** and **747/1593 Mbps for 4K**, rounded up. These come from desktop and game test scenes on AMD, including SDR/HDR and 4:2:0/4:4:4; picture content still decides how clean any one stream looks. The one-pixel text at 720p needed more bits per pixel than the larger text at higher resolutions. [Criteria, rates and limits](configuration.md#capture-and-video) explain the two levels.

Raise the bitrate in Moonlight on a fast wired network with headroom for packet overhead and recovery data. 4K60 at the recommended rate needs more than gigabit Ethernet. If the client or network cannot carry it, use HEVC or AV1. Compare the same scene after changing the rate; the warning does not diagnose packet loss or HDR rendering. NVIDIA users should use [Vibepollo](https://github.com/Nonary/Vibepollo).

## Low latency is reported, but motion still stutters

Compare the same moving scene at the same resolution, frame rate, codec and bitrate. A static desktop may legitimately send repeated or fewer frames. Begin with 1080p60 SDR, then change one variable.

| Statistic | What it helps distinguish |
| --- | --- |
| **Overview → Frame rate** | Frames sent by the host. Repeated pictures can still count toward this rate. |
| **Encode** (p95) | Slower conversion/encoding completions, including asynchronous encoder work. |
| **Host processing** | Moonlight's reported interval. Since 2.4.8 it is the capture/encode output tick through entry into packetization, matching Punktfunk's software path. Prepared-packet queueing and pacing are separate; asynchronous encode time is available as **Encoder time**. |
| **Frame age** | Waiting from the capture/presentation timestamp until the host claims the frame. |
| **Present to sent** | New pictures, from Windows' present to the last packet sent: capture, the wait for the claim, encoding and sending. Compare this, not Moonlight's host latency, with another host's present-to-sent figure. |
| Moonlight's network and decoder statistics | Loss, jitter or client decoding delays after host processing. |

None of the host timings measures the complete input-to-screen delay. Smoothness also depends on distinct pictures arriving regularly. The [performance guide](performance.md) separates host timings, measured picture age, fresh-picture rate and arrival gaps.

If the problem starts when the GPU is fully occupied, compare with a lower game frame cap or lighter graphics settings. If client network loss rises, reduce bitrate or compare a wired connection. If decoder time rises, reduce resolution/rate or select another hardware-decoded codec. Keep WGC and compute defaults for the baseline; use capture/compute overrides only for a recorded comparison. The published local tests do not establish results for every Radeon model or Wi-Fi connection.

## The picture freezes while audio and input continue

Update to [2.2.18](../rust/RELEASE_NOTES.md#new-in-2218) if a busy shared capture gets stuck on an old picture after the desktop stops changing. Earlier versions could consume the final desktop update while every snapshot buffer was occupied. The host now retains the pending update until consumers release space. This specific fix does not diagnose an encoder or GPU-driver hang.

If you explicitly use **Software** with HEVC, 2.2.18 also fixes recovery requests that produced ordinary intra pictures instead of decoder-resetting IDRs. This can leave a client unable to recover after reference loss on earlier versions.

**Try AV1 first** when the client can decode it in hardware, then HEVC.

Start from the default encoder settings: `amd_usage=ultralowlatency`, `amd_lowlatency_mode=auto`, `amd_input_queue_size=0`, `amd_preanalysis=disabled`, `amd_split_frame=auto` and `compute_queue_realtime=false`. Use `amd_smart_access_video=disabled` while diagnosing a freeze. Install AMD's current Recommended driver for your card from [AMD's driver page](https://www.amd.com/en/support/download/drivers.html).

If the freeze returns, change one setting per reconnect (WGC against Desktop Duplication, then `gpu_compute_conversion` on against off) and note which one changes the result.

Send the host log **from `CLIENT CONNECTED` through `CLIENT DISCONNECTED`**, plus startup/probe failures and the client log around the failure time. The default **Information** log level already writes encoder settings, display setup and the 5-second `stream timings` lines. **Debug** (`min_log_level=debug`) adds GPU scheduling and probe details; the change applies right away. Preserve whole lines, including fields. In particular include:

- `stream configured`, `capture backend opened`, `compute queue ready for colour conversion` and, at debug level, `GPU process scheduling configured`.
- `AMF encoder settings`, `AMF split-frame encoding left to the driver` or `AMF split-frame encoding requested`, and every `AMF setting unavailable` / `AMF split-frame request not accepted` warning. The HEVC engine field should report `HevcNumOfHwInstances=1` on the reported hardware; send the actual value. Include `LowLatencyInternal`, `HevcInputQueueSize`, `HevcEnableEncoderSmartAccessVideo`, `HevcEnablePreAnalysis`, and, for AV1, `Av1InputQueueSize`, `Av1EnableEncoderSmartAccessVideo`, `Av1EnablePreAnalysis` and `Av1BPicturesPattern`. `?` means a property could not be read, not that it is disabled.
- `AMF SmartAccess Video disabled because forced low latency can reset the GPU` if an imported configuration triggered the guard.
- All `stream timings` lines, especially `fps`, `encode_p95_ms`, `encode_p99_ms`, `frame_age_p95_ms`, `present_to_sent_ms` and the `stage_*_ms` fields (mean/p95/p99 of each stage), `send_interval_p99_ms`, `idr_requests`, `reference_invalidations` and `bitrate_kbps`, alongside Moonlight's network loss and decoder statistics.
- Any `GPU device lost: DXGI_ERROR_DEVICE_REMOVED`, `GPU work did not finish within two seconds`, `capture copies stopped completing`, `Encoding failed (…); recreating the same encoder`, `Encoder output failed (…); recreating the same encoder`, `The encoder returned no frame for … ms; recreating the same encoder` or `Encoder recreation failed` lines. Include `AMF compute conversion unavailable (…); converting on the graphics queue` and `PyroWave compute conversion unavailable (…); converting on the graphics queue` if present.
- For PyroWave, include `optional codec probe` errors; for an OS/driver reset, include the matching Reliability Monitor/Windows event time and LiveKernelEvent code. A frozen picture with continuing `stream timings` and no recovery warning is useful evidence too.

## The picture freezes and the GPU resets during a stream

Windows logs a `LiveKernelEvent` 141 (VIDEO_ENGINE_TIMEOUT_DETECTED) and the
Radeon may stay in Device Manager error Code 31 until a reboot. This is the
AMD driver's video encoder hanging inside an encode call. Rubylight's stall
watch then logs `stream thread stopped making progress` with
`phase=encoding`; its GPU query probe may answer or wait about 3 s while
Windows resets the engine. About 34 s later the encode call returns
`DXGI_ERROR_DEVICE_RESET`. No host process can end a call blocked in the
kernel driver, and ending the stream does not stop the reset.

On the test RX 7900 XT this happened with H.264, HEVC and AV1, SDR and HDR,
with and without long-term references, at 63 to 399 Mb/s, on drivers
32.0.31041.1004 and 32.0.32015.2008, and every time the driver then failed
to bring the adapter back (live kernel events 193 and 1B0, Code 31).

If it happens on your PC:

- Reboot if the Radeon shows Code 31.
- Send the report the AMD Bug Report Tool offers after the reset; it carries
  the driver's own hang record, which only AMD can read.
- Keep the bitrate at what the link can carry, so the client needs fewer
  recovery keyframes. On Wi-Fi, use AV1 when the client decodes it: it
  recovers from loss with a small frame instead of a keyframe.
- Try AMD's current Recommended driver and report the driver version, the
  time of the `LiveKernelEvent`, and the host log around it.

## Playnite does not launch

Keep a Windows user signed in, enable Playnite in **Settings → Game library**, and check that the game starts in Playnite locally. If Rubylight cannot find a portable copy, leave Playnite open in that user's session when starting the stream.

In **Library**, check Playnite's plugin status. Use **Install plugin** or **Update plugin** if offered, fully exit and reopen Playnite, then try **Sync now**. Use the connector bundled with Rubylight; newer Vibepollo connectors may require different host support. A `Playnite CLI fallback requested` warning means automatic game-exit tracking is unavailable; quit that stream manually.

**Long installation paths:** 2.2.18 can report Windows error 3 (path not found) even when the plugin folder exists, because its temporary filename exceeds the legacy Windows path limit. [2.2.19](../rust/RELEASE_NOTES.md#new-in-2219) supports these paths, including Unicode names, while preserving atomic replacement and partial-install detection.

Set **Log level** to **Debug** before reproducing so the log includes `Playnite plugin check` and the launch-pipe details. Send the failure time, installed or portable location, Desktop or Fullscreen mode, and the host log from `Playnite launch prepared` and `Playnite plugin check` through the pipe, startup, fallback or exit messages. Include any `Moonlight session launch failed`, `Playnite library sync failed`, `Playnite startup failed`, `Playnite did not confirm startup` or `Playnite game exit confirmed` lines. If available, include Playnite's `playnite.log` and the matching `%APPDATA%\Sunshine\logs\sunshine_playnite-*.log`.

## RTSS does not start, or the game ignores the cap

Open **Maintenance → Frame limiter** during the affected stream. Record **Configured**, **Active now**, RTSS detection/running state and **RTSS folder**. A missing tray icon alone does not establish whether the limiter is active.

In **Settings → Frame limiting**, check **Limiter**, **Frame limit**, **Virtual display refresh** and **RTSS folder**. **None** disables limiting. Physical-monitor streams need **Limit every stream** if you want a cap on every stream; virtual-display policy can apply its own cap. A frame limit of `0` follows the stream rate, including fractional values such as 59.94.

The folder must contain the RTSS executable and its hook library, not just a shortcut. If the log says RTSS requires administrator privileges, start RTSS as administrator before streaming or use Rubylight's installed service. If it is found and running but one game ignores the limit, check that game's RTSS profile and include the game/API in the report. If RTSS's window keeps your own limit during a stream, Rubylight could not reach the elevated RTSS: use the installed service, or run a portable copy as administrator. RTSS 7.3.7 ignores edits of its settings file and takes a new limit only through its own interface, which Rubylight uses from rc.27; the log line "RTSS frame limit applied and verified" names the RTSS version.

Rubylight restores limiter values after the last stream owning the limit disconnects. A game retaining its display must not keep the cap active. If a cap remains, check for another pending or connected stream and keep the before/during/after values with the log.

## Monitors stay on, or the display layout does not return

Check **Settings → Display** and app/device overrides first. **Extended** intentionally keeps physical displays alongside the virtual display. Exclusive and isolated layouts have different behavior. A client may also request its own virtual-display arrangement.

Check **Restore displays on disconnect**, **Restore delay** and **Keep a disconnected display for** against the expected lifecycle. An app or retained remote-monitor session may still own a display after video disconnects.

**Restore displays on disconnect** defaults to off. To return to the physical monitors when Moonlight disconnects while a game or Desktop session remains open, turn it on; the default restore delay is three seconds. Otherwise the game display is retained for up to two hours, or until the app is quit. A paused game should leave local Win+P changes alone while keeping its display available for reconnection. Resuming reactivates that display and reapplies the stream layout. A separate remote-monitor session intentionally keeps its monitor available until disconnected.

When all streams have ended, use **Maintenance → Displays** to inspect whether the current layout matches the saved one. **Restore saved layout** applies that saved layout; **Save current layout** replaces it with the arrangement you currently want. Saving, restoring and resetting are unavailable during streaming.

**Disconnect virtual displays** stops every stream and removes Rubylight-created virtual displays. **Reset display settings memory** forgets pending display changes that Rubylight would otherwise undo. Use those recovery actions deliberately after recording the problem; resetting memory is not the same as restoring a layout.

rc.10 has a brief startup guard for a reproduced case where creating a virtual display reactivated a dormant monitor. Record the client, chosen layout, active monitors and log time when reporting another case.

## "Virtual display did not become active before the deadline"

Up to rc.10, a launch failed with this error when Windows left the new virtual display switched off, typically because it recalled a layout saved for the other connected displays (two TVs in duplicate mode, for example). rc.11 switches the display on itself after a second, without changing the other displays, and logs `Windows left the new virtual display switched off; switched it on beside the current displays`. Update before trying workarounds such as unplugging the TVs. If a launch still fails, the error says whether Windows reported the display as connected; include that line and the log around it in the report.

## The stream disconnects after a second with "os error 10035"

`video sender stopped: A non-blocking socket operation could not be completed immediately. (os error 10035)` comes from rc.1, which ended the stream when the Windows send buffer was momentarily full. Since rc.2 the host waits briefly, drops only those packets and keeps streaming, as Vibepollo does; the log then shows `UDP send failed; packets dropped` at most every five seconds. Install the current release. Frequent drop warnings mean the network cannot carry the bitrate: lower it, or use a standard codec rather than PyroWave over Wi-Fi.

## A controller or other input does nothing

Check the stream card in the console first.

- **"…its device permissions do not allow it"**: the device lacks that input permission. Every device paired after the first starts view-only, as in Apollo and Vibepollo. Turn on **Controllers** (or Touch, Pen, Mouse, Keyboard) for it under **Devices**, then reconnect.
- **"Virtual gamepad driver unavailable"**: the VHF gamepad driver could not be opened. Run the installer again with the gamepad driver selected.

## Steam shows two controllers

Some Steam builds can list one VHF Xbox controller twice. Steam's SDL controller discovery races its XInput and GameInput backends for the same device. Start+Select may then open both Xbox Game Bar and Steam's keyboard. SDL has an [upstream fix](https://github.com/libsdl-org/SDL/commit/c4cfb739). Users report that the Steam beta, which includes it, lists the pad once; switch to it under Steam → Settings → Interface → Client Beta Participation. The fix will reach Steam's stable client in a later update.

Steam's `logs/controller.txt` shows each arrival. Rubylight no longer uses ViGEmBus, which earlier releases offered as a workaround; an installed copy can be uninstalled.

## A Steam Deck has no gyro, trackpads or back grips

The host only follows what Moonlight announces when a controller connects. With Steam Input on for Moonlight, Steam on the Deck gives Moonlight a virtual pad, which has no gyro, trackpads or back grips. On the Deck, disable Steam Input in Moonlight's controller settings in Steam, then reconnect. The stream card says this when the paired device is named Steam Deck and its controller has no gyro. In Rubylight's logs, a Deck that Moonlight sees directly connects with `client_type=4`.

- **Steam on the host doesn't show a Steam Deck:** the host attaches one only with usbip-win2 installed and, with **Automatic**, while Steam is running on the host when the controller connects; otherwise the log says `Steam is not running` and the Deck gets a virtual DualSense. The log line `Steam Deck controller attached through usbip-win2` names the usbip-win2 port, and `usbip.exe port` lists it. If the stream card says the attach failed, its message is usbip.exe's own.
- **No trackpads with Steam Input off:** Moonlight builds whose SDL predates SDL 3 do not read the Deck's trackpads, so they send none.
- **Back grips do nothing on the virtual pad:** choose what each grip presses under **Settings → Input → Steam Deck and back grips**; the virtual pads have no back buttons of their own. A Deck attached as a Steam Deck keeps its own grips, and Steam Input on the host decides what they do.
- **A Steam Deck controller left behind after a crash:** usbip-win2 detaches it when Rubylight's connection closes, then keeps trying to attach it again. `usbip.exe attach --stop-all` stops that.
- **Shutdown hangs after a failed USB/IP attachment:** 2.2.18 depends on a second loopback connection to wake its idle controller server. If Windows blocks that connection, teardown can wait indefinitely. [2.2.19](../rust/RELEASE_NOTES.md#new-in-2219) wakes the server without making a connection. A working loopback connection is still required to attach the controller.

See [Steam Deck](configuration.md#steam-deck).

## Setup rejects the profile folder's owner

An upgrade from 2.2.1 can fail with `C:\ProgramData\Butterpollo must be owned by Administrators, SYSTEM or TrustedInstaller`. Older installers set the folder's access permissions but could leave an individual user as its owner. Changing access permissions alone does not change ownership.

The [fix in 2.2.6](../rust/RELEASE_NOTES.md#new-in-226) repairs that legacy ownership and the profile permissions during a verified in-place service upgrade, keeping settings and paired devices. Credentials remain restricted to SYSTEM and Administrators. Use the 2.2.6 or later installer and upgrade in the existing installation folder.

Setup still rejects reparse points (including links and junctions) and untrusted parent folders. The repair does not apply to a fresh install or to a damaged installation whose missing or unreadable package manifest forces a full reinstall. If setup still refuses the path, keep the profile backup and include the setup log from [the locations below](#logs-and-a-useful-report) in your report.

## An update says Rubylight cannot be updated in its folder

The Rubylight service runs as SYSTEM from the install folder, so setup only updates a folder that no one but Administrators, SYSTEM or TrustedInstaller owns. Installers up to 2.2.1 locked a folder outside Program Files down without taking it from the user who created it, and setup since 2.2.2 then refused every update there.

- **The install folder itself belongs to a user** (for example `D:\Rubylight`, created in Explorer): an update now gives it to Administrators and locks it down again, as it does for the profile folder, and carries on.
- **A folder above it belongs to a user** (for example `D:\Games` for `D:\Games\Rubylight`): that user could put another folder in its place, so Rubylight cannot stay there. **Maintenance → Updates** says so. Run the installer from the release page: it offers `C:\Program Files\Rubylight` (or another folder through **Change folder…**) and moves Rubylight there, keeping the service, settings, paired devices and drivers. If Rubylight does not start from the new folder, setup returns the service to the old one. Afterwards nothing uses the old folder; delete it yourself. A quiet setup without `--install-dir` moves to `C:\Program Files\Rubylight` as well.

## An update stays queued

In **Maintenance → Updates**, read the current phase and any error. Installation needs the installed Windows service and one minute without streams, pending connections, remote monitors or host apps. Quit a retained Desktop/game session as well as disconnecting its video. A new connection defers the update.

**Install updates automatically** is opt-in. **Include pre-releases** controls whether candidates appear, and an update-check interval of `0` disables automatic checks and installation. Manual checks remain available. Portable builds use the release-page download.

## Logs and a useful report

Use **Logs → Download log** for the host log or **Download support bundle** for a ZIP containing logs, configuration diagnostics and an available crash dump. Review the contents before posting publicly.

| File | Default location |
| --- | --- |
| Installed host log | `%PROGRAMDATA%\Butterpollo\config\logs\butterpollo.log` |
| Installed service log | `%PROGRAMDATA%\Butterpollo\config\logs\service.log` |
| Portable host log | Adjacent `config\logs\butterpollo.log` (2.2.6 or later); otherwise `%LOCALAPPDATA%\ButterpolloRust\config\logs\butterpollo.log` |
| Setup log | `%TEMP%\butterpollo-setup-<timestamp>.log` |

A custom `--config-dir` changes the profile location; `log_path` can override the host log path. The support-bundle download is named `butterpollo-support.zip`.

The release includes `tools\collect_environment.ps1`. From PowerShell in the installed or extracted package folder, collect read-only environment evidence with:

```powershell
.\tools\collect_environment.ps1 -InstallDirectory . -OutputFile "$env:TEMP\butterpollo-environment.json"
```

Choose a new output filename if that file already exists. The collector records Windows, GPU/driver, physical network-adapter counters, host version/hash and service state. It does not start capture, change settings or upload the report. A source checkout has the same script at [`rust/tests/collect_environment.ps1`](../rust/tests/collect_environment.ps1).

For a display-specific report, `butterpollo.exe --diagnostics` prints the detected displays and virtual-driver status without starting a stream. In a portable/user context, its driver-access result may differ from the installed service.

Open an [issue](https://github.com/RamazanKara/Rubylight/issues) with the exact symptom and time, host/client versions, GPU and driver, wired or Wi-Fi connection, resolution/FPS/codec/HDR settings, and the relevant log or support bundle. Include the environment report when the result seems hardware-specific.
