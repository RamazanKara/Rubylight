# Troubleshooting

[Documentation](README.md) · [Getting started](getting-started.md) · [Configuration](configuration.md)

Start with the stream card on **Home**, **Help → Log analysis** and the host log. Note the time of the failure before changing anything, then change one setting at a time. Capture and performance probes add GPU work, so compare them in an otherwise idle session.

| Area | Symptoms |
| --- | --- |
| Install and start | [This PC cannot run Rubylight](#setup-or-the-host-says-this-pc-cannot-run-rubylight) · [Console or port](#the-console-will-not-open-or-a-port-is-occupied) · [Profile folder owner](#setup-rejects-the-profile-folders-owner) · [Install folder](#an-update-says-rubylight-cannot-be-updated-in-its-folder) · [Queued update](#an-update-stays-queued) · [Drivers](#a-driver-is-missing-or-out-of-date) |
| Connecting | [Pairing](#moonlight-cannot-find-or-pair-with-the-pc) · [Outside your network](#clients-outside-your-network-cannot-connect) · [Browser stream or guest link](#the-browser-stream-or-a-guest-link-fails-to-connect) · [os error 10035](#the-stream-disconnects-after-a-second-with-os-error-10035) · [Drops and timeouts](#the-stream-breaks-up-drops-or-times-out-on-the-network) |
| Picture | [Stream card warnings](#what-the-stream-card-warnings-mean) · [Codec not available](#the-client-asks-for-a-codec-this-gpu-cannot-encode) · [Black screen](#black-picture-no-display-or-wgc-fails) · [HDR colour](#hdr-looks-washed-out-too-bright-or-different-between-clients) · [Blurred PyroWave](#pyrowave-shows-blurred-grey-blocks) · [Stutter](#low-latency-is-reported-but-motion-still-stutters) · [Frozen picture](#the-picture-freezes-while-audio-and-input-continue) · [GPU reset](#the-picture-freezes-and-the-gpu-resets-during-a-stream) |
| Displays | [Layout not restored](#monitors-stay-on-or-the-display-layout-does-not-return) · [Mode refused](#windows-refuses-the-virtual-displays-requested-mode) · [Display not active](#virtual-display-did-not-become-active-before-the-deadline) |
| Sound and input | [Audio](#audio-cuts-out-or-lags) · [Controllers](#a-controller-or-other-input-does-nothing) · [reWASD](#rewasd-or-another-tool-doesnt-see-the-virtual-controller) · [Two controllers in Steam](#steam-shows-two-controllers) · [DualSense keeps vibrating](#a-dualsense-keeps-vibrating-after-steam-stops-it) · [Xbox button prompts](#games-show-xbox-button-prompts-with-a-dualsense) · [Steam Deck](#a-steam-deck-has-no-gyro-trackpads-or-back-grips) |
| Games | [Playnite](#playnite-does-not-launch) · [RTSS](#rtss-does-not-start-or-the-game-ignores-the-cap) · [Nightjar host control](#nightjar-cannot-close-a-game-or-change-radeon-settings) |
| Reporting | [Logs and a useful report](#logs-and-a-useful-report) |

## Setup or the host says this PC cannot run Rubylight

Rubylight encodes on an AMD Radeon GPU through the AMF video encoder in AMD's driver, or on an NVIDIA GPU through NVENC in NVIDIA's driver, on Windows 10 or 11. Setup checks before it installs or updates anything, and the host checks at startup and for every stream.

| Message | What it means and what to do |
| --- | --- |
| "...this PC has only Intel ..." | There is no AMD or NVIDIA GPU. Use [Rubylight 2.6.x](https://github.com/RamazanKara/Rubylight/releases/tag/v2.6.0), which supports Intel graphics. |
| "...Windows lists no graphics card with a working driver..." | Windows shows no graphics driver, only the basic display adapter. If the PC has a Radeon or an NVIDIA GPU, install its graphics driver from amd.com/support or nvidia.com/drivers, then run setup again. On Intel graphics, use 2.6.x. |
| "...its driver has no AMF video encoder (amfrt64.dll is missing...)" | The AMD GPU is there, but its driver lacks the encoder. Install the latest AMD Radeon graphics driver (the full Adrenalin package, not only Windows Update's basic driver), restart Windows if it asks, then run setup again. |
| "...its driver has no NVENC video encoder (nvEncodeAPI64.dll is missing...)" | The NVIDIA GPU is there, but its driver lacks the encoder. Install the latest NVIDIA graphics driver from nvidia.com/drivers, restart Windows if it asks, then run setup again. |
| "AMF could not start on ..." in the console | The AMD driver is present but AMF did not start. Update the Radeon driver. At the sign-in screen, or after a disconnected Remote Desktop session, sign in at the PC first. |
| "NVENC could not start on ..." in the console | The NVIDIA driver is present but NVENC did not start. Update the NVIDIA driver. At the sign-in screen, or after a disconnected Remote Desktop session, sign in at the PC first. |
| "... is neither an AMD nor an NVIDIA graphics card ... This PC also has ..." | A monitor or the **Graphics card** setting points at the Intel GPU of a PC that also has an AMD or NVIDIA GPU. Connect the monitor to that GPU, choose it under **All settings → Picture → Encoder → Graphics card**, or switch to WGC capture (Desktop Duplication captures on the GPU that drives the monitor), and reconnect. |

A Moonlight client that cannot start a stream on such a PC shows "No video encoder is ready" with the same advice. The console's warnings and `butterpollo.log` hold the specific reason.

## The console will not open, or a port is occupied

The default base port is `47989`; the console uses `47990`. A changed **Base port** moves the console to the next port up. Check the active profile's `sunshine.conf` if the launcher reports another address.

**Start Rubylight.exe** reopens the correct console when that profile is already running. If it reports another streaming host on the port, close that host or the second Rubylight instance, or give each instance [its own profile and ports](configuration.md#multiple-instances). Restarting another copy on the same port does not solve the conflict.

For an installed host, check **Rubylight** in Windows Services. Its internal service name is `ApolloService`, or `RubylightService` when it is [installed alongside another host](getting-started.md#keep-another-host-and-install-alongside); that installation's console is at the port setup reported, by default `48590`. If it is stopped, read `service.log` before starting it again. An update keeps the startup type you set for the service (Manual or Disabled stay as they are; setup does not start a disabled service), and **Start Rubylight.exe** offers to start a stopped service after a Windows permission prompt. The [log locations below](#logs-and-a-useful-report) distinguish service and portable profiles.

## Setup rejects the profile folder's owner

Installers up to 2.2.1 set the profile folder's access permissions but could leave an individual user as its owner. Changing access permissions alone does not change ownership, and an upgrade then fails with `C:\ProgramData\Butterpollo must be owned by Administrators, SYSTEM or TrustedInstaller`.

Setup repairs that ownership and the profile permissions during an in-place service upgrade, keeping settings and paired devices. Credentials remain restricted to SYSTEM and Administrators. Upgrade in the existing installation folder.

Setup still rejects reparse points (including links and junctions) and untrusted parent folders. The repair does not apply to a fresh install, or to a damaged installation whose missing or unreadable package manifest forces a full reinstall. If setup still refuses the path, keep the profile backup and include the setup log from [the locations below](#logs-and-a-useful-report) in your report.

## An update says Rubylight cannot be updated in its folder

The Rubylight service runs as SYSTEM from the install folder, so setup only updates a folder that no one but Administrators, SYSTEM or TrustedInstaller owns. The system drive's root (`C:\`) may also belong to an account in the Administrators group, for example after taking its ownership in Explorer or through a tweak, because whoever owns it could already change Program Files. When setup refuses a folder, the error names the owner it found.

- **The install folder itself belongs to a user** (for example `D:\Rubylight`, created in Explorer, or locked down by an installer up to 2.2.1 without taking it from the user who created it): an update gives it to Administrators, locks it down again as it does for the profile folder, and carries on.
- **A folder above it belongs to a user** (for example `D:\Games` for `D:\Games\Rubylight`): that user could put another folder in its place, so Rubylight cannot stay there. **Settings → Updates** says so. Run the installer from the release page: it offers `C:\Program Files\Rubylight` (or another folder through **Change folder…**) and moves Rubylight there, keeping the service, settings, paired devices and drivers. If Rubylight does not start from the new folder, setup returns the service to the old one. Afterwards nothing uses the old folder; delete it yourself. A quiet setup without `--install-dir` moves to `C:\Program Files\Rubylight` as well.

## An update stays queued

Under **Settings → Updates**, read the current phase and any error; the panel names the stream an update waits for, and **Install now** installs without waiting. Installation needs the installed Windows service and one minute without streams, pending connections, remote monitors or host apps. Quit a retained desktop or game session as well as disconnecting its video. A new connection defers the update.

**Install updates automatically** (**Settings → Updates**) is opt-in. **Include pre-releases** (**All settings → This PC → Updates**) controls whether release candidates are offered, and an update-check interval of `0` disables automatic checks and installation. Manual checks remain available. Portable builds update through the release page.

## A driver is missing or out of date

- **Rubylight's display driver:** "Rubylight's display driver is not installed", "the virtual display driver is missing" or "the Rubylight display driver speaks protocol ..." means virtual displays are unavailable until the driver is installed and current. Restart Windows if setup asked for it; otherwise run Rubylight's setup again, which reinstalls the driver. The service also sets the driver up again a minute after it finds it missing, unless a stream is running.
- **AMD's graphics driver:** "AMFInit refused this host's AMF version", "AMF setting unavailable", "AMF LTR unavailable" or "DuplicateOutput1 unavailable" means the driver lacks a feature the host asks for. The stream runs with the driver's defaults. Install the current AMD Radeon driver (the full Adrenalin package, not only Windows Update's driver) and restart Windows if it asks.

## Moonlight cannot find or pair with the PC

- Open **https://localhost:47990** on the host. If it does not open, check the host or service first.
- Confirm both devices are on the same local network, then try adding the host's local IP address in Moonlight. Guest networks and access-point isolation can keep devices from reaching each other.
- Check **All settings → Network → Pairing → Allow pairing**. Enter the four-digit PIN **shown by Moonlight** into the matching pending request on **Devices**.
- If discovery alone fails, check the discovery and listen-address settings in **All settings → Network** (**Discovery and internet**, and the collapsed **Ports and addresses**). An address restricted to `127.0.0.1` is reachable only from the host itself.
- If Home in the console warns that a Windows Firewall rule blocks Rubylight, a block rule for `butterpollo.exe` (left when Windows asked whether to allow it and the prompt was closed) overrides setup's allow rule. Open Windows Defender Firewall, **Advanced settings**, **Inbound Rules**, and allow `butterpollo.exe` or delete the rule that blocks it.
- If setup reported a firewall-rule failure, read the setup log for the exact error and check that Windows Firewall allows the installed `butterpollo.exe` on your local network. Do not disable the whole firewall to diagnose one application.

If the device pairs but cannot launch Desktop, open the device on **Devices** and check that it is enabled and has the **List apps**, **View streams** and **Launch apps** permissions. A device without mouse or keyboard permission can receive video without that input working.

## Clients outside your network cannot connect

With `upnp` on, the host asks the router to forward its streaming ports, through UPnP, else PCP, else NAT-PMP. **Settings → Network → Internet status** shows which worked, each port the router refused and why, and notes for carrier-grade NAT, where forwarded ports cannot be reached from outside ([Play over the internet](internet-play.md) then gives Nightjar a direct path). "UPnP mapping failed" or "UPnP mapping timed out" means the router refused or did not answer: turn UPnP on in the router, or forward the ports by hand. "UPnP port is already owned by another mapping" means an older mapping, often from another streaming host, holds the port: remove it in the router's port forwarding list. Devices on the same network do not need UPnP.

## The browser stream or a guest link fails to connect

The picture and sound reach a browser over UDP, on a port of their own (`webrtc_media_port`, 8001 by default; guests use the next four). The page says what it saw:

| The page says | What it means and what to do |
| --- | --- |
| "This browser does not stream from this PC's loopback address" | Firefox on the PC itself, with a host that only listens on `127.0.0.1`. Firefox makes no attempt to reach a loopback address. Remove `--bind 127.0.0.1`, or use Chrome or Edge. |
| "This browser did not try to reach the PC" (or "Your browser did not try") | The browser sent nothing. A setting or extension that limits WebRTC (an ad blocker's WebRTC protection, `media.peerconnection.ice.proxy_only` in Firefox), or a VPN, usually causes it: turn it off for this page. |
| "The PC did not answer at *address*, UDP port *port*" | The browser sent to that address and nothing came back within 8 seconds. Allow `butterpollo.exe` or that UDP port through the firewall on the PC, including other security software. Setup adds a Windows Firewall rule for the program, so a block usually comes from something else. Check that no VPN on the device takes the traffic away from the PC. |
| "Your browser could not reach the PC on UDP port *port*" | The same, for a guest. Forward that UDP port to the PC ([ports](browser-guests.md)), or connect over Tailscale. |
| "...reached the PC, but the encrypted connection to it failed" | The PC answered and the DTLS handshake failed, or hadn't finished 30 seconds later. Try again. If it keeps failing, antivirus or a firewall that inspects traffic may be in the way. |
| "The connection to the PC was lost" | The stream had connected and then stopped getting through. Check the network between the device and the PC. |

In the host log, `WebRTC offer's host candidate` and `guest offer's host candidate` give the address that was offered, `connection failed:` gives what the Stream page saw and the browser (one line per attempt, at most one in 10 seconds across the host), and `the stream ended before its media connected` gives how many datagrams reached the media port. Zero datagrams means they never reached the host.

## The stream disconnects after a second with "os error 10035"

`video sender stopped: A non-blocking socket operation could not be completed immediately. (os error 10035)` comes from an early 3.0 pre-release, which ended the stream when the Windows send buffer was momentarily full. The current host waits briefly, drops only those packets and keeps streaming; the log then shows `UDP send failed; packets dropped` at most every five seconds. Install the current release. Frequent drop warnings mean the network cannot carry the bitrate: lower it, or use a standard codec rather than PyroWave over Wi-Fi.

## The stream breaks up, drops or times out on the network

- **"client control stream timed out" or "client audio ping timed out":** the device stopped sending on the stream's UDP ports. It lost Wi-Fi, went to sleep, closed the app, or a firewall dropped the traffic. Keep the device awake on the same network, and check that Windows Firewall allows `butterpollo.exe` on private networks; setup adds that rule, but other security software can block it.
- **"Video frames were lost on the way to this device", "could not carry" or "video frames are not arriving":** packets did not reach the device, usually on Wi-Fi or a busy link. The host paces the video or lowers the bitrate to get it through. Lower the bitrate in the client, use 5 GHz Wi-Fi closer to the access point, or connect the device by cable.
- **"roaming reply not sent":** the device moved to another address during the stream, and the host could not answer it there. Reconnect once the device has settled on its new network.

## What the stream card warnings mean

The stream card shows the active encoder and warnings for the current session. Each warning also writes a `WARN` log line with its cause and a next step. Capture recovery and audio capture warnings clear when those paths recover. Dropped packets, frames and input, and audio device changes, stay on the card for 30 seconds after they last happened. A warning describes a fallback or limitation, not a measured latency penalty.

- **Encoder failure:** AMF on Radeon and NVENC on NVIDIA are the only encoders for H.264, HEVC and AV1. If the GPU's encoder cannot start, the stream fails with its error in the log; there is no software encoder to fall back to. Check that error and the graphics driver.
- **Graphics queue conversion or copies:** compute setup failed. A busy game can delay capture or conversion. Lower the game's GPU load or check the graphics driver.
- **Desktop Duplication or capture interrupted:** WGC or its helper could not capture. UAC and lock screens can cause a temporary change. Unlock the desktop; if the warning persists, check the helper error. A display size change reopens capture.
- **Physical display, refresh, HDR or limiter:** the requested setup is unavailable, unsupported or owned by another stream. Check the actual display mode and RTSS. A 60 Hz source cannot supply 116 fresh pictures per second.
- **Audio:** a missing virtual sink can send sound to the host's speakers. Device changes and capture failures can pause sound. The host-loss warning names what was lost: sound Windows dropped because the host's audio sender read too late (a busy CPU or a host problem, not bitrate), or audio packets the network adapter refused (lower the video bitrate or check the adapter).
- **Bitrate, pacing or FEC:** the encoder or wire budget was limited, packets were dropped, or large frames lost FEC protection. Check **Maximum bitrate** and the client bitrate. Runtime bitrate updates are capped at 500 Mbps. Leave headroom for audio, packet headers and FEC.
- **AMF rate-control limit rejected:** the driver accepted the target bitrate but refused a peak-rate, buffer or frame-size limit. Later bitrate changes retry using that limit's last accepted scaling basis; an unreadable property does not clear its warning. If it persists, reconnect at the desired bitrate or update the AMD driver.
- **Input:** the selected driver or Windows input API is unavailable. Video continues; repair the named driver or choose a supported profile.
- **PyroWave:** the client negotiated another codec, or bitrate or recovery protection is insufficient. Use a PyroWave-capable client and a fast wired network, or choose HEVC or AV1.

## The client asks for a codec this GPU cannot encode

`AMF_CODEC_NOT_SUPPORTED`, `AMF_ENCODER_NOT_PRESENT` or "this GPU or driver has no encoder for the request" in the log means the device asked for something this GPU's video encoder does not have: a codec (AV1 on a Radeon without an AV1 encoder, for example), HDR with H.264, a YUV 4:4:4 mode the host did not offer, or a size or frame rate beyond the encoder's limit. The checks under **PC details** on **Home** list the codecs, and on NVIDIA the 4:4:4 modes, this GPU encodes. Choose one of those in the client's video settings, or a lower resolution or frame rate, and reconnect. If a codec the GPU should have is missing, update the graphics driver.

## Black picture, no display, or WGC fails

1. In the checks under **PC details** on **Home**, inspect **Screen capture**, **Video encoder** and **Virtual display**. A physical monitor must be active, or the configured virtual display must be available.
2. Check **All settings → Picture → Streamed display → Display** and any app or device override. Confirm that the selected display is the one showing the desktop or game.
3. For WGC under the installed service, keep a Windows user signed in. Rubylight starts its WGC capture worker in that user's session. A locked or UAC desktop uses the Desktop Duplication recovery path; WGC is retried when the normal desktop returns. A stream started while Windows is locked sets up its display on the lock screen; if the virtual display cannot be set up there, the stream shows the physical display and the stream card says so. Start the stream again after signing in.
4. Read the `capture backend opened` log entry for the backend that actually opened. `requested_capture=wgc` describes the request and can appear even when capture falls back.
5. If the virtual-display status reports access denied, use the installed Windows service and check that the bundled driver is ready. A portable host with administrator rights does not have the service's access to the driver.

For a controlled comparison, disconnect first, set **All settings → Picture → Capture → Capture method** to **Desktop Duplication**, reconnect at the same resolution and rate, and record whether the symptom changes. Return to **Automatic** afterwards unless you need the explicit choice. Keep the original failure log so a successful fallback does not hide the WGC error.

The installed service always runs the host in the session attached to the physical console. When Remote Desktop takes over a signed-in desktop, Windows gives the console a new session at the sign-in screen, and the host moves there. Closing the Remote Desktop window leaves that desktop disconnected, and a Moonlight stream then shows the console's sign-in screen. Signing in there as the same user makes Windows reconnect the disconnected desktop to the console and end the sign-in session, which also ends that stream. Reconnect Moonlight once the desktop is back; the host restarts in that session within a few seconds. The host logs a warning naming the disconnected session when a stream starts in this state.

For a host started inside the Remote Desktop session (not the installed service), open **Advanced → Troubleshoot → Displays and Windows session → Return to console**. With the installed service the host already runs on the console, so this action is unavailable and its status explains why. The action returns only the Windows session running this host to the physical console: its desktop becomes visible at the PC and an active RDP connection disconnects. Disconnect Moonlight streams first. The console must have no signed-in user, or hold a disconnected session whose Windows user SID matches the host session's user. Active sessions, different users and unverified identities stay protected. The host may restart while Windows switches sessions; refresh the status once it returns, then reconnect Moonlight. If Windows cannot verify both users or refuses the operation, use the installed service or sign in at the PC. The action never asks for a Windows password.

Launch requests check the host's own Windows session before and after preparation, and again before RTSP playback, so a disconnect during display or app preparation returns a retry error instead of accepting the pending stream. The log records the session ID, console ID, connection state, protocol and acceptance reason, so a Windows classification problem can be told apart from a transition during preparation; a refused check is logged as a warning. These checks cannot prevent Windows from switching sessions after playback has started.

## HDR looks washed out, too bright, or different between clients

Separate the host's HDR source, the negotiated stream and the client's display output:

- Check **Windows Settings → System → Display** for the **streamed display**. HDR on another monitor does not mean HDR on this one, and wide-gamut SDR is not HDR.
- Check the client's HDR option and the Home stream card's **HDR** badge. In the host log, `hdr=true`, the codec and `source_pixel=RgbaF16` identify a negotiated HDR stream with floating-point capture. FP16 capture alone does not prove that the game produces HDR highlights.
- Start with **All settings → Picture → HDR → Display HDR: Match the stream** and **HDR request: Follow the device**. A display-only override does not change what the client negotiated. Check app and device overrides too.
- Some HDR TVs wash the picture out when the stream's HDR metadata carries no light levels (`MaxCLL` and `MaxFALL`). Windows reports only the display's levels, so since 4.3.3 Rubylight sends the display's peak and full-frame brightness when the content reports none; update an older host first.
- Confirm the game's HDR setting and the receiving display's HDR mode. To compare two clients, keep the codec, scene and host display the same; AV1 HDR on one device against HEVC SDR on another is not an equivalent check.
- Report whether black levels, mid-grey menus, bright highlights or colour saturation are wrong. Include the client app and version, device and display model, codec, stream resolution and rate, and the host log time.

Every client receives the HDR state once per stream ([#11](https://github.com/RamazanKara/Rubylight/issues/11)), so a TV without an HDR badge has not negotiated HDR. When the display's colour space changes during a stream, the host updates its colour information and reopens capture; changed HDR metadata is sent with a keyframe, and transient AMD metadata failures are retried. If HDR was turned on after the stream started and the picture still looks wrong, reconnect once with HDR already on for the streamed display and compare the same scene.

`AMF output range mismatch` or `AMF full-range output cannot be established` stops the encoder instead of letting it produce the wrong range. Reconnect with limited range or update the AMD driver. Optional input-range properties that older drivers reject are not fatal.

For a report, include the log from stream startup, especially `stream configured`, `source_pixel`, `hdr`, `full_range` and any AMF range or HDR-metadata warnings; a timing-only excerpt cannot establish the negotiated colour path. Rubylight's HEVC and AV1 HDR path is checked down to reference pixels; see the [native HDR record](../rust/PERFORMANCE.md#final-native-virtual-hdr-pixels-excluding-physical-panel-calibration).

## PyroWave shows blurred grey blocks

A very low bitrate can leave PyroWave with only coarse brightness and little colour or fine detail. The stream card and log distinguish `PyroWave bitrate is too low` (red, below the severe-loss floor) from `PyroWave bitrate is below recommended` (amber, text and textures may lose detail).

At 60 fps the floor and recommendation are **139/277 Mbps for 720p**, **187/399 Mbps for 1080p** and **747/1593 Mbps for 4K**, rounded up. They come from desktop and game test scenes on AMD, including SDR and HDR, 4:2:0 and 4:4:4; picture content still decides how clean any one stream looks. One-pixel text at 720p needs more bits per pixel than the larger text at higher resolutions. [Capture and video](configuration.md#capture-and-video) explains the two levels.

Raise the bitrate in Moonlight on a fast wired network with headroom for packet overhead and recovery data. 4K60 at the recommended rate needs more than gigabit Ethernet. If the client or network cannot carry it, use HEVC or AV1. Compare the same scene after changing the rate; the warning does not diagnose packet loss or HDR rendering.

## Low latency is reported, but motion still stutters

Compare the same moving scene at the same resolution, frame rate, codec and bitrate. A static desktop may legitimately send repeated or fewer frames. Begin with 1080p60 SDR, then change one variable.

| Statistic | What it helps distinguish |
| --- | --- |
| **Home → Frame rate** | Frames sent by the host. Repeated pictures can count toward this rate. |
| **Encode** (p95) | Slower conversion and encoding completions, including asynchronous encoder work. |
| **Host processing** | Moonlight's reported interval: from the capture and encode output tick to entry into packetization. Prepared-packet queueing and pacing are separate; asynchronous encode time is shown as **Encoder time**. |
| **Frame age** | Waiting from the capture or presentation timestamp until the host claims the frame. |
| **Present to sent** | New pictures, from Windows' present to the last packet sent: capture, the wait for the claim, encoding and sending. Compare this, not Moonlight's host latency, with another host's present-to-sent figure. |
| Moonlight's network and decoder statistics | Loss, jitter or client decoding delays after host processing. |

None of the host timings measures the complete input-to-screen delay. Smoothness also depends on distinct pictures arriving regularly. The [performance guide](performance.md) separates host timings, measured picture age, fresh-picture rate and arrival gaps.

If the problem starts when the GPU is fully occupied, compare with a lower game frame cap or lighter graphics settings. If client network loss rises, reduce the bitrate or compare a wired connection. If decoder time rises, reduce resolution or rate, or select another hardware-decoded codec. Keep the default capture and compute settings for the baseline; use capture and compute overrides only for a recorded comparison. The published measurements cover the Radeon GPUs and networks listed in the [hardware matrix](hardware-matrix.md), not every model or Wi-Fi connection.

## The picture freezes while audio and input continue

**Try AV1 first** when the client decodes it in hardware, then HEVC.

On a Radeon, start from the default encoder settings: `amd_usage=ultralowlatency`, `amd_lowlatency_mode=auto`, `amd_input_queue_size=0`, `amd_preanalysis=false`, `amd_split_frame=auto` and `compute_queue_realtime=false`. Use `amd_smart_access_video=disabled` while diagnosing a freeze. Install AMD's current Recommended driver for your card from [AMD's driver page](https://www.amd.com/en/support/download/drivers.html).

If the freeze returns, change one setting per reconnect (WGC against Desktop Duplication, then `gpu_compute_conversion` on against off) and note which one changes the result.

Send the host log **from `CLIENT CONNECTED` through `CLIENT DISCONNECTED`**, plus startup and probe failures and the client log around the failure time. The default **Information** log level already writes encoder settings, display setup and the 5-second `stream timings` lines. **Debug** (`min_log_level=debug`) adds GPU scheduling and probe details and applies right away. Keep whole lines, including their fields. In particular include:

- `stream configured`, `capture backend opened`, `compute queue ready for colour conversion` and, at debug level, `GPU process scheduling configured`.
- `AMF encoder settings`, `AMF split-frame encoding left to the driver` or `AMF split-frame encoding requested`, and every `AMF setting unavailable` or `AMF split-frame request not accepted` warning. The HEVC engine field should report `HevcNumOfHwInstances=1` on the reported hardware; send the actual value. Include `LowLatencyInternal`, `HevcInputQueueSize`, `HevcEnableEncoderSmartAccessVideo`, `HevcEnablePreAnalysis`, and, for AV1, `Av1InputQueueSize`, `Av1EnableEncoderSmartAccessVideo`, `Av1EnablePreAnalysis` and `Av1BPicturesPattern`. `?` means a property could not be read, not that it is disabled.
- `AMF SmartAccess Video disabled because forced low latency can reset the GPU` if an imported configuration triggered the guard.
- All `stream timings` lines, especially `fps`, `encode_p95_ms`, `encode_p99_ms`, `frame_age_p95_ms`, `present_to_sent_ms` and the `stage_*_ms` fields (mean, p95 and p99 of each stage), `send_interval_p99_ms`, `idr_requests`, `reference_invalidations` and `bitrate_kbps`, alongside Moonlight's network loss and decoder statistics.
- Any `GPU device lost: DXGI_ERROR_DEVICE_REMOVED`, `GPU work did not finish within two seconds`, `capture copies stopped completing`, `Encoding failed (…); recreating the same encoder`, `Encoder output failed (…); recreating the same encoder`, `The encoder returned no frame for … ms; recreating the same encoder` or `Encoder recreation failed` lines. Include `AMF compute conversion unavailable (…); converting on the graphics queue` and `PyroWave compute conversion unavailable (…); converting on the graphics queue` if present.
- For PyroWave, include `optional codec probe` errors; for an OS or driver reset, include the matching Reliability Monitor or Windows event time and LiveKernelEvent code. A frozen picture with continuing `stream timings` and no recovery warning is useful evidence too.

## The picture freezes and the GPU resets during a stream

Windows logs a `LiveKernelEvent` 141 (VIDEO_ENGINE_TIMEOUT_DETECTED), and the Radeon may stay in Device Manager error Code 31 until a reboot. This is the AMD driver's video encoder hanging inside an encode call. Rubylight's stall watch then logs `stream thread stopped making progress` with `phase=encoding`; its GPU query probe may answer, or wait about 3 s while Windows resets the engine. About 34 s later the encode call returns `DXGI_ERROR_DEVICE_RESET`. No host process can end a call blocked in the kernel driver, and ending the stream does not stop the reset.

On the project's RX 7900 XT test host this happened with H.264, HEVC and AV1, SDR and HDR, with and without long-term references, at 63 to 399 Mb/s, on drivers 32.0.31041.1004 and 32.0.32015.2008, and each time the driver then failed to bring the adapter back (live kernel events 193 and 1B0, Code 31).

If it happens on your PC:

- Reboot if the Radeon shows Code 31.
- Send the report the AMD Bug Report Tool offers after the reset; it carries the driver's own hang record, which only AMD can read.
- Keep the bitrate at what the link can carry, so the client needs fewer recovery keyframes. On Wi-Fi, use AV1 when the client decodes it: it recovers from loss with a small frame instead of a keyframe.
- Try AMD's current Recommended driver and report the driver version, the time of the `LiveKernelEvent` and the host log around it.

## Monitors stay on, or the display layout does not return

Check **All settings → Picture → Streamed display** and app or device overrides first. The **Extend** layouts intentionally keep physical displays alongside the virtual display; exclusive and isolated layouts behave differently. A client may also request its own virtual-display arrangement.

Check **Restore displays on disconnect**, **Restore delay** and **Keep a disconnected display for** against how long you want displays kept. An app or a retained remote-monitor session may still own a display after video disconnects.

**Restore displays on disconnect** is on by default, with a three-second restore delay. Exclusive layouts restore after this delay even when an older profile turns the setting off, unless **Keep monitors off while paused** is on: then, with **Restore displays on disconnect** off, **Keep a disconnected display for** decides how long the monitors stay off, or until the app closes. A **Restore hotkey**, once you set one, brings them back at any time. The final stream restores the original displays even if you used Win+P to regain local access. With restoring turned off, non-exclusive layouts can be retained for reconnection and keep local layout changes while paused. Resuming a retained display reactivates it and reapplies the stream layout. A separate remote-monitor session keeps its monitor until it disconnects.

After sleep, the virtual-display driver keeps its adapter and monitor identities and recreates capture resources. If Windows marks the Rubylight driver as failed, a new stream or an active display's recovery can request a device restart. Recovery is limited to this driver, uses bounded connection retries, and allows three restarts with a one-minute cooldown. It does not enable a manually disabled device or restart Windows.

When all streams have ended, use **Advanced → Troubleshoot → Displays and Windows session** to see whether the current layout matches the saved one. **Restore saved layout** applies the saved layout; **Save current layout** replaces it with the arrangement you have now. Saving, restoring and resetting are unavailable during a stream.

**Disconnect virtual displays** stops every stream and removes the virtual displays Rubylight created. **Reset display settings memory** forgets pending display changes that Rubylight would otherwise undo; it is not the same as restoring a layout. Record the problem before using either.

The host guards against creating a virtual display waking a dormant monitor. If a monitor still turns on, report the client, chosen layout, active monitors and log time.

## Windows refuses the virtual display's requested mode

`virtual display mode not applied` reports the requested and actual resolution and refresh, the rates enumerated at that resolution, and the Windows error. `Windows refused the requested display mode (-2)` means `DISP_CHANGE_BADMODE`; it does not say why Windows excluded the mode. An empty `offered_hz` list means the host enumerated no modes at that resolution; the support log alone cannot tell filtering from an enumeration failure.

For VRR the host requests a 1000 Hz virtual display. The driver's EDID declares no separate vertical, horizontal or pixel-clock maximum that would rule out a mode such as 4K at 1000 Hz, and leaves mode support to IddCx ([#40](https://github.com/RamazanKara/Rubylight/issues/40)).

To capture Windows' reason, run [trace_display_modes.ps1](../rust/tests/trace_display_modes.ps1) in an administrator PowerShell, then start the failing stream during its 45-second capture. It uses a private, size-limited IddCx trace and does not change driver, display or service settings. Keep the resulting `IddCx.etl`, `environment.json` and a support bundle from the same reproduction. The WPP messages need matching Microsoft public symbols to decode; see [Microsoft's IddCx debugging guide](https://learn.microsoft.com/windows-hardware/drivers/display/indirect-display-debugging). Capture the failure before changing settings so the evidence describes the original problem.

## "Virtual display did not become active before the deadline"

Windows can leave a new virtual display switched off, typically because it recalls a layout saved for the other connected displays (two TVs in duplicate mode, for example). The host then switches it on after a second without changing the other displays, and logs `switched the new virtual display on beside the current displays`. If a launch still fails with this error, the error says whether Windows reported the display as connected; include that line and the log around it in the report.

## Audio cuts out or lags

Audio and video share the network path. On Wi-Fi, a video burst can fill the adapter's or access point's queue even when the average bitrate looks reasonable, and lost HEVC or AV1 frames can then require a larger recovery frame.

1. Lower the client's bitrate and compare the same scene. HEVC or AV1 is a better starting point for Wi-Fi than PyroWave. Compare a wired connection when possible.
2. A fast host Ethernet connection does not mean the client or the rest of the path can carry equally fast bursts. For a known constrained path, compare a fixed `pacing_max_bitrate_kbps` below its capacity but above the stream's wire demand, including FEC. For example, `80000` is a reasonable comparison for a 40 Mbps stream with 20% FEC on a 100 Mbps path. This adds send time without lowering the picture bitrate. Save and reconnect, then compare the same scene and loss statistics. Explicit limits keep a 110% encoder-bitrate floor; they are not raised automatically for FEC.
   When keyframes themselves are lost on such a path, the picture freezes on every recovery. The host then paces the rest of the stream at twice the encoder bitrate and the stream card says so (**Slow down after video loss**, on by default). A fixed limit avoids the first frozen moments too. Clients that report a Wi-Fi or cellular route, such as Nightjar, are paced adaptively from the first frame (**Adaptive packet pacing for Wi-Fi clients**, on by default).
3. **Adaptive packet pacing** (`host_adaptive_pacing`) is an alternative when the suitable rate is unknown. Leave the pacing limit at `0` and turn it on. It starts conservatively, then probes faster rates while video is active and control feedback is healthy; a confirmed loss during a probe restores the previous rate. Startup is slower, and probing can still lose packets. It is off by default and does not apply with a positive fixed limit, host adaptive bitrate or PyroWave. If loss continues, lower the client's bitrate too; pacing cannot create network capacity.

Compare Moonlight's loss statistics and the host log before and after. `reference_invalidations` counts reference-loss feedback separately from `idr_requests`; neither counts lost packets. PyroWave has no inter-frame references, so its recovery feedback does not force another encode or increase FEC. A steadily rising request counter alone does not establish Wi-Fi loss. [Capture and video](configuration.md#capture-and-video) has the pacing details, and the [transport record](../rust/PERFORMANCE.md#october-7-pyrowave-recovery-feedback) the limits of the local tests.

## A controller or other input does nothing

Check the stream card in the console first.

- **"…its device permissions do not allow it"**: the device lacks that input permission. Devices paired after the first get the permissions chosen at pairing (**Watch only** has no input), or view-only permissions when no choice was made. Turn on **Controllers** (or Touch, Pen, Mouse, Keyboard) for it under **Devices**, then reconnect.
- **"Virtual gamepad driver unavailable"**: the virtual gamepad driver could not be opened. Run the installer again with the gamepad driver selected.
- **"Xbox 360 (ViGEmBus) controller unavailable"** or **"DualShock 4 (ViGEmBus) controller unavailable"**: that controller type is chosen and ViGEmBus could not be opened. The installer doesn't bring it: install [ViGEmBus](https://github.com/nefarius/ViGEmBus/releases), or choose another controller type.

## reWASD or another tool doesn't see the virtual controller

Windows lists the standard pads as HID devices of a virtual driver, not as USB devices, and some tools only look for USB controllers. Choose **Xbox 360 (ViGEmBus)** or **DualShock 4 (ViGEmBus)** as the [controller type](configuration.md#controllers), with ViGEmBus installed: those pads are emulated USB devices. The DualShock 4 keeps the touchpad, motion and lightbar, so a tool's lightbar colour reaches the controller in your hand. If the tool still doesn't list it, open an issue naming the tool and its version.

## Steam shows two controllers

Some Steam builds list one virtual Xbox controller twice: Steam's SDL controller discovery races its XInput and GameInput backends for the same device. Start+Select may then open both Xbox Game Bar and Steam's keyboard. SDL has an [upstream fix](https://github.com/libsdl-org/SDL/commit/c4cfb739), and users report that the Steam beta, which includes it, lists the pad once; switch to it under Steam → Settings → Interface → Client Beta Participation until the stable client carries the fix.

Steam's `logs/controller.txt` shows each arrival. Rubylight uses ViGEmBus only when you choose Xbox 360 (ViGEmBus) or DualShock 4 (ViGEmBus) as the [controller type](configuration.md#controllers); otherwise an installed copy can be uninstalled.

## A DualSense keeps vibrating after Steam stops it

With the virtual DualSense from the virtual gamepad driver, Steam's rumble stop (after **Identify Controller**, for example) can be lost, because the driver reports it as the previous rumble. Until the driver is fixed, use one of these:

- Run the installer again with **Install USB audio and haptics support** selected and choose **DualSense with waveform haptics (USB/IP)** as the [controller type](configuration.md#controllers). That DualSense stops when Steam stops it.
- Choose **DualShock 4** as the controller type.

## Games show Xbox button prompts with a DualSense

With the DualSense [controller type](configuration.md#controllers), Windows sees a HID device with Sony's ID 054c:0ce6, and the host log says `virtual controller connected … profile="vhf_ds5"`. Games that read it directly can show PlayStation prompts; a game that understands only Xbox controllers can't see it at all. So a game that responds but shows Xbox prompts either reads the pad generically and defaults to Xbox icons, or something on the host has wrapped it as an Xbox controller. Check in this order:

- **Steam Input:** for a game started through Steam, Playnite launches of Steam games included, Steam Input hands a PlayStation pad to the game as a virtual Xbox 360 controller. Big Picture doesn't change that. Choose **Disable Steam Input** under the game's **Properties → Controller** in Steam: a game with its own DualSense support then sees the pad and switches prompts. A game without it stops seeing the controller; turn Steam Input back on for that one and pick the icon style in the game.
- **Other remappers:** DS4Windows, DSX or a similar tool on the host can add its own Xbox pad. During a stream, Windows' Game Controllers panel (`joy.cpl`) should list only **Wireless Controller**.
- **USB DualSense:** some games only treat a controller as a DualSense when it is a USB device, as a cabled one is. Run the installer again with **Install USB audio and haptics support** selected on the **Steam Deck and DualSense USB audio and haptics** page, then choose **DualSense with waveform haptics (USB/IP)** as the controller type. Windows then sees a USB DualSense with its audio interface. It works with any Moonlight client; waveform haptics need a client that sends them ([DualSense waveform haptics](dualsense-haptics.md)). Connect to Desktop first, wait for the controller, then start the game, and switch Windows' sound output back if it moves to the controller.
- **Order:** some games choose their prompts once, at launch. Connect the controller before starting the game.

If none of these help, the game shows PlayStation prompts only through its own Sony-specific path; its icon setting is then the way to change them.

## A Steam Deck has no gyro, trackpads or back grips

The host only follows what Moonlight announces when a controller connects. With Steam Input on for Moonlight, Steam on the Deck gives Moonlight a virtual pad, which has no gyro, trackpads or back grips. On the Deck, disable Steam Input in Moonlight's controller settings in Steam, then reconnect. The stream card says this when the paired device is named Steam Deck and its controller has no gyro. In Rubylight's logs, a Deck that Moonlight sees directly connects with `client_type=4`.

- **Steam on the host doesn't show a Steam Deck:** the host attaches one only with usbip-win2 installed and, with **Automatic**, only while Steam is running on the host when the controller connects; otherwise the log says `Steam is not running` and the Deck gets a virtual DualSense. The log line `Steam Deck controller attached through usbip-win2` names the usbip-win2 port, and `usbip.exe port` lists it. If the stream card says the attach failed, its message is usbip.exe's own.
- **No trackpads with Steam Input off:** Moonlight builds whose SDL predates SDL 3 do not read the Deck's trackpads, so they send none.
- **Back grips do nothing on the virtual pad:** choose what each grip presses under **All settings → Sound & controls → Steam Deck and back grips**; the virtual pads have no back buttons of their own. A Deck attached as a Steam Deck keeps its own grips, and Steam Input on the host decides what they do.
- **A Steam Deck controller left behind after a crash:** usbip-win2 detaches it when Rubylight's connection closes, then keeps trying to attach it again. `usbip.exe attach --stop-all` stops that. Attaching the controller needs a working loopback connection on the host.

See [Steam Deck](configuration.md#steam-deck).

## Playnite does not launch

Keep a Windows user signed in, turn on **All settings → Games → Playnite**, and check that the game starts in Playnite locally. If Rubylight cannot find a portable copy, leave Playnite open in that user's session when starting the stream.

On **Games**, check Playnite's plugin status. Use **Install plugin** or **Update plugin** if offered, fully exit and reopen Playnite, then try **Sync now**. Use the plugin bundled with Rubylight; a plugin taken from another streaming host may expect different host support. A `Playnite CLI fallback requested` warning means automatic game-exit tracking is unavailable; quit that stream manually.

Set **Log level** to **Debug** before reproducing so the log includes `Playnite plugin check` and the launch-pipe details. Send the failure time, installed or portable location, Desktop or Fullscreen mode, and the host log from `Playnite launch prepared` and `Playnite plugin check` through the pipe, startup, fallback or exit messages. Include any `Moonlight session launch failed`, `Playnite library sync failed`, `Playnite startup failed`, `Playnite did not confirm startup` or `Playnite game exit confirmed` lines. If available, include Playnite's `playnite.log` and the matching `%APPDATA%\Sunshine\logs\sunshine_playnite-*.log`.

## RTSS does not start, or the game ignores the cap

Open **All settings → Picture → Integrations → Frame limiter** during the affected stream. Record **Configured**, **Active now**, the RTSS detection and running state, and **RTSS folder**. A missing tray icon alone does not show whether the limiter is active.

In **All settings → Picture**, check **Limiter**, **Frame limit** and **Virtual display refresh** in the **Frame limiter** group, and **RTSS folder** in the collapsed **RTSS** group. **None** turns limiting off. Physical-monitor streams need **Limit every stream** for a cap on every stream; virtual-display policy can apply its own cap. A frame limit of `0` follows the stream rate, including fractional values such as 59.94.

The folder must contain the RTSS executable and its hook library, not just a shortcut. If the log says RTSS requires administrator privileges, start RTSS as administrator before streaming or use Rubylight's installed service. If it is found and running but one game ignores the limit, check that game's RTSS profile and include the game and graphics API in the report. If RTSS's window keeps your own limit during a stream, Rubylight could not reach the elevated RTSS: use the installed service, or run a portable copy as administrator. RTSS 7.3.7 ignores edits of its settings file and takes a new limit only through its own interface, which Rubylight uses; the log line `RTSS frame limit applied and verified` names the RTSS version.

Rubylight restores limiter values after the last stream owning the limit disconnects, and a game keeping its display does not keep the cap active. If a cap remains, check for another pending or connected stream and keep the before, during and after values with the log.

## Nightjar cannot close a game or change Radeon settings

These need the **Host control** permission for that device, which is off until you turn it on. Open **Devices**, open the device, expand **Custom permissions**, and tick **Host control** under **Other**. The same applies to NVIDIA driver settings on a host that streams with an NVIDIA graphics card. Close game also needs the **Launch apps** permission ([host control](configuration.md#host-control-for-a-device)). Radeon changes also need the device to own a running or suspended session; a second device sees the session but cannot act on it.

If a Radeon setting is greyed out, its state says why: some need a one-time setup in AMD Software, and some the GPU or driver does not offer. If it says the driver is busy or ADLX did not load, try again later or update the Radeon driver. On NVIDIA the panel-wide reason is that NVAPI did not load; update the NVIDIA driver. An NVIDIA value set with a profile editor that the panel cannot put back shows as read only. A change from the client that was not put back after the session ended is logged in `butterpollo.log` with the setting's id and retried at the next host start. A setting you changed on the PC in the meantime is left as you set it ([how restore works](configuration.md#radeon-settings-with-restore)).

## Logs and a useful report

**Help → Log analysis** reads the newest part of the host log on the host itself and explains the encoder, capture, network and driver failures it recognises, each with how often and when it last happened and a link to its section here. It sends nothing anywhere.

Use **Logs → Download log** for the host log, or **Download support bundle** (on **Help** and **Logs**) for a ZIP with logs, configuration diagnostics and any crash dump. Review the contents before posting them publicly.

| File | Default location |
| --- | --- |
| Installed host log | `%PROGRAMDATA%\Butterpollo\config\logs\butterpollo.log` |
| Installed service log | `%PROGRAMDATA%\Butterpollo\config\logs\service.log` |
| Portable host log | `config\logs\butterpollo.log` beside `butterpollo.exe` when that `config` folder exists; otherwise `%LOCALAPPDATA%\ButterpolloRust\config\logs\butterpollo.log` |
| Setup log | `%TEMP%\butterpollo-setup-<timestamp>.log` |

A custom `--config-dir` changes the profile location; `log_path` can override the host log path. The support-bundle download is named `butterpollo-support.zip`.

The release includes `tools\collect_environment.ps1`. From PowerShell in the installed or extracted package folder, collect read-only environment evidence with:

```powershell
.\tools\collect_environment.ps1 -InstallDirectory . -OutputFile "$env:TEMP\butterpollo-environment.json"
```

Choose a new output filename if that file already exists. The collector records Windows, GPU and driver, physical network-adapter counters, host version and hash, and service state. It does not start capture, change settings or upload anything. A source checkout has the same script at [`rust/tests/collect_environment.ps1`](../rust/tests/collect_environment.ps1).

For a display-specific report, `butterpollo.exe --diagnostics` prints the detected displays and virtual-driver status without starting a stream. In a portable or user context its driver-access result can differ from the installed service's.

Open an [issue](https://github.com/RamazanKara/Rubylight/issues) with the exact symptom and time, host and client versions, GPU and driver, wired or Wi-Fi connection, resolution, frame rate, codec and HDR settings, and the relevant log or support bundle. Include the environment report when the result seems hardware-specific.
