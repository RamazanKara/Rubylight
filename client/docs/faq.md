# FAQ

[Documentation](index.md)

## What do I need?

A device running Android 5.0 or newer, Windows 10 or 11, or Linux with glibc 2.39 or newer; a PC running a Moonlight-compatible streaming host such as Sunshine, Apollo, Vibepollo or Rubylight; and a network between them. Start with the [quick start](quick-start.md). Some features, such as PyroWave, HDR formats and audio passthrough, have further device or host requirements, listed with each feature.

## Does Nightjar run the game on this device?

No. The game runs on your PC. Nightjar shows its video, plays its audio and sends your controls back.

## Does it work without Rubylight?

Yes. Nightjar works with any Moonlight-compatible host and enables each feature from what the host reports. Rubylight 3.0 adds features that need its extensions, such as Home, sharing and co-op; [host feature support](PARITY-MATRIX.md) lists which features need what.

## Can I keep Moonlight installed?

Yes. Nightjar has its own app ID and pairs separately, so it installs beside Moonlight and other clients. It imports Artemis on-screen keyboard layouts and Custom Keys files ([import from Artemis](controls.md#import-from-artemis)).

## Does it need root?

The normal `nonRoot` build does not. A separate root flavor exists for older Android mouse-capture use; see [building](building.md#flavors-and-outputs).

## Why do filenames or package names contain a different identifier?

Nightjar was earlier called Butterpollo and then Rubylight Android. The Android package ID remains `com.butterpollo.client` so existing installations keep their identity, and Nightjar installs over Rubylight Android, keeping its PCs, pairing and settings. The ES-DE export folder stays `butterpollo`, and some internal identifiers keep their earlier names; [the rename history](RENAME.md) lists them. The product and repository are Nightjar.

## Can I stream outside my home network?

Yes, when this device can reach the host's streaming ports. Pair on the local network first, then connect through a private network such as Tailscale or your own VPN, and add the PC's address on that network. [Play away from home](remote-setup.md) walks through it.

## Where do I set device permissions?

In your host's settings or web UI, if it has per-device permissions, as Apollo, Vibepollo and Rubylight do. Choose **Open host web UI** in an online PC's settings and sign in with the web UI's account. Listing apps, viewing, launching, each input type, clipboard directions and commands have separate permissions. See [pairing](pairing.md#per-device-permissions).

## Does Disconnect close my game?

Disconnect ends the local stream and leaves the host app running. Select it again to resume, or turn on **Quick resume** in Settings to resume with one tap on the PC. Quit game requests host application exit and needs permission and confirmation; it is in the stream menu and in the running game's library menu. To quit a game every time you disconnect, turn on **Quit the game when I leave the stream** in that game's library menu. A lost connection never quits the game. See [leaving a game](controls.md#leaving-a-game).

## Why did changing a preset do nothing?

A PC's own values override the preset for that PC. In that PC's settings, set them to Same as all PCs and save, then start a new stream. [Settings](settings.md) lists every preset write and reset scope.

## Which codec or upscaler should I choose?

Begin with the Automatic codec and the default upscaling. Compare measured decode time and image quality on your device before changing them. [Video and latency](video-and-latency.md) explains the choices, HDR and VRR.

## Why do I see banding in gradients and skies?

8-bit video can show visible steps in smooth gradients. If your screen has no HDR, or you keep HDR off, turn on **10-bit color (SDR)** in Stream settings. It needs a Main10 HEVC or AV1 decoder on this device and a PC that offers 10-bit; otherwise the stream stays 8-bit and Nightjar says so. The PC's display must be in SDR. See [HDR and color](video-and-latency.md#hdr-and-color).

## Is the overlay latency the delay I feel?

Statistics shows network round-trip time, available host processing, decode time and display wait separately. Debug adds detailed stage timings. These values do not measure physical input sampling through display scanout; physical end-to-end latency needs an external measurement. Choose the view through **stream menu → Display → Overlay and diagnostics**.

## Can I use my launcher?

Nightjar exports game files for ES-DE and accepts Android intents from Daijisho, Pegasus and other frontends. See [frontends](FRONTENDS.md).

## Does an update preserve pairing?

An update signed with the same key normally preserves the installation. Uninstalling, clearing app storage or restoring onto another device requires fresh pairing. A differently signed APK cannot update the installed app.

## Does Nightjar upload diagnostics automatically?

No. On Android, problem reports stay local until you choose a receiving app in the share sheet. The desktop client writes a local log that you attach yourself. See [report export](TROUBLESHOOTING.md#export-a-problem-report) and [privacy](PRIVACY.md) for network checks, storage and backups.
