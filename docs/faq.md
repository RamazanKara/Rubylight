# FAQ

[Documentation](index.md)

## What do I need?

An Android 5.0+ device, a PC running a Moonlight-compatible streaming host (such as Apollo, Rubylight, Sunshine or Vibepollo) and a working network between them. Start with the [quick start](quick-start.md). Individual HDR, USB and Vulkan features have additional device requirements.

## Does Nightjar run the game on Android?

The game runs on your PC. Nightjar streams its video/audio to Android and sends your controls back.

## Does it need root?

The normal `nonRoot` build does not. A separate root flavor exists for older Android mouse-capture use; see [building](building.md#flavors-and-outputs).

## Why do filenames or package names contain a different identifier?

Nightjar was earlier called Butterpollo and then Rubylight. The Android package ID remains `com.butterpollo.client` so existing installations keep their identity. The ES-DE export folder stays `butterpollo`, and some internal identifiers keep their earlier names; [the rename history](RENAME.md) lists them. The product and repository are Nightjar.

## Can I stream outside my home network?

Yes, when your Android device can reach the host's streaming services. Establish local pairing first, then configure a routed connection such as your own VPN. Add the host's reachable address manually for that route.

## Where do I set device permissions?

In your host's settings or web UI, if it has per-device permissions (e.g. Apollo, Rubylight or Vibepollo). Choose **Open host web UI** in an online PC's settings and sign in with the web UI's account. Listing apps, viewing, launching, each input type, clipboard directions and commands have separate permissions. See [pairing](pairing.md#per-device-permissions).

## Does Disconnect close my game?

Disconnect ends the local stream and leaves the host app running. Select it again to resume, or turn on **Quick resume** in Settings to resume with one tap on the PC. Quit game requests host application exit and needs permission and confirmation; it is in the stream menu and in the running game's library menu. To quit a game every time you disconnect, turn on **Quit the game when I leave the stream** in that game's library menu. A lost connection never quits the game. See [leaving a game](controls.md#leaving-a-game).

## Why did changing a preset do nothing?

A PC's own values override the preset for that PC. In that PC's settings, set them to Same as all PCs and save, then start a new stream. [Settings](settings.md) lists every preset write and reset scope.

## Which codec or upscaler should I choose?

Begin with Automatic codec and upscaling Off. Compare measured decode time and image quality on your device before changing them. [Video and latency](video-and-latency.md) explains the choices, HDR and VRR.

## Why do I see banding in gradients and skies?

8-bit video can show visible steps in smooth gradients. If your screen has no HDR, or you keep HDR off, turn on **10-bit color (SDR)** in Stream settings. It needs a Main10 HEVC or AV1 decoder on this device and a PC that offers 10-bit; otherwise the stream stays 8-bit and Nightjar says so. The PC's display must be in SDR. See [HDR and color](video-and-latency.md#hdr-and-color).

## Is the overlay latency the delay I feel?

Statistics shows network round-trip time, available host processing, decode time and display wait separately. Debug adds detailed stage timings. These values do not measure physical input sampling through display scanout; physical end-to-end latency needs an external measurement. Choose the view through **stream menu → Display → Overlay and diagnostics**.

## Can I use my launcher?

Nightjar exports game files for ES-DE and accepts Android intents from Daijisho, Pegasus and other frontends. See [frontends](FRONTENDS.md).

## Does an update preserve pairing?

An update signed with the same key normally preserves the installation. Uninstalling, clearing app storage or restoring onto another device requires fresh pairing. A differently signed APK cannot update the installed app.

## Does Nightjar upload diagnostics automatically?

No. Problem reports are local until you choose a receiving app in Android's share sheet. See [report export](TROUBLESHOOTING.md#export-a-problem-report) and [privacy](PRIVACY.md) for network checks, storage and backups.
