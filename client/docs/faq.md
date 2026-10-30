# FAQ

[Documentation](index.md)

## What do I need?

A device with Android 5.0 or newer, Windows 10 or 11, or x86-64 Linux with glibc 2.39 or newer; a PC running a Moonlight-compatible host such as Sunshine, Apollo, Vibepollo or Rubylight; and a network between them. Start with the [quick start](quick-start.md). Some features, such as PyroWave, the HDR formats and audio passthrough, have further device or host requirements, listed with each feature.

## Does Nightjar run the game on this device?

No. The game runs on the PC. Nightjar shows its video, plays its audio and sends your input back.

## Does it work without Rubylight?

Yes. Nightjar works with any Moonlight-compatible host and enables each feature from what the host reports. Some features need host extensions that Rubylight 3.0 provides, such as Home, sharing and co-op; [host feature support](PARITY-MATRIX.md) lists what each feature needs.

## Can I keep Moonlight installed?

Yes. Nightjar has its own application ID and pairs separately, so it installs beside Moonlight and other clients. It can import Artemis on-screen keyboard layouts and Custom Keys files ([import from Artemis](controls.md#import-from-artemis)).

## Does it need root?

No. The standard `nonRoot` build needs no root access. A separate root flavor exists for mouse capture on older Android versions; see [building](building.md#flavors-and-outputs).

## Why do filenames or package names contain a different identifier?

Nightjar was previously called Butterpollo and then Rubylight Android. The Android package ID remains `com.butterpollo.client`, so existing installations keep their identity and Nightjar installs over Rubylight Android with its PCs, pairing and settings. The ES-DE export folder is still `butterpollo`, and some internal identifiers keep their earlier names; [the rename history](RENAME.md) lists them.

## Can I stream outside my home network?

Yes, when this device can reach the host's streaming ports. Pair on the local network first, then connect through a private network such as Tailscale or your own VPN, and add the PC by its address on that network. [Play away from home](remote-setup.md) walks through it.

## Where do I set device permissions?

In the host's web UI, on hosts with per-device permissions such as Apollo, Vibepollo and Rubylight. Nightjar opens it from an online PC's settings (**Open host web UI** on Android, **Host web UI** on the desktop); sign in with the web UI's own account. Listing apps, viewing, launching, each input type, each clipboard direction and commands are separate permissions. See [pairing](pairing.md#per-device-permissions).

## Does Disconnect close my game?

No. **Disconnect** ends this device's stream and leaves the game running on the PC; select it again to resume, or turn on **Quick resume** to resume with one tap on the PC. **Quit game**, in the stream menu and in the running game's menu in the library, asks the host to end the game and needs permission and confirmation. To quit a game every time you leave it, turn on **Quit the game when I leave the stream** in that game's menu. A lost connection never quits the game. See [leaving a game](controls.md#leaving-a-game).

## Why did changing a preset do nothing?

The PC has its own values, which take precedence over the preset for that PC. In the PC's settings, set them to **Same as all PCs**, save, and start a new stream. [Settings](settings.md) lists what each preset writes and what each reset clears.

## Which codec or upscaler should I choose?

Start with the automatic codec and the default upscaling. Change them only after comparing decode time and image quality on your device. [Video and latency](video-and-latency.md) explains the choices, HDR and VRR.

## Why do I see banding in gradients and skies?

8-bit video can show visible steps in smooth gradients. If the screen has no HDR, or you keep HDR off, turn on **10-bit color (SDR)** in the stream settings. It needs a Main10 HEVC or AV1 decoder on this device and a PC that offers 10-bit; otherwise the stream stays 8-bit and Nightjar says so. The PC's display must be in SDR. See [HDR and color](video-and-latency.md#hdr-and-color).

## Is the overlay latency the delay I feel?

Not entirely. **Statistics** shows network round trip, host processing, decode time and display wait separately, and **Debug** adds detailed stage timings. None of them covers the full path from input sampling to display scan-out; that needs an external measurement. Choose the view under **stream menu → Display → Overlay and diagnostics**.

## Can I use my launcher?

Yes. Nightjar exports game files for ES-DE and accepts launches from Daijisho, Pegasus, Beacon and other Android frontends; see [launcher integrations](FRONTENDS.md). On Windows and Linux, `nightjar --host <PC> --app <game>` starts a game from any launcher.

## Does an update preserve pairing?

Yes, for an update signed with the same key. Uninstalling, clearing app storage or restoring onto another device requires pairing again. An APK signed with a different key cannot update the installed app.

## Does Nightjar upload diagnostics automatically?

No. On Android, a problem report stays on the device until you choose an app in the share sheet. The desktop client writes a redacted local log that you attach yourself. See [exporting a problem report](TROUBLESHOOTING.md#export-a-problem-report), and [privacy](PRIVACY.md) for network checks, storage and backups.
