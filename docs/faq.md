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

In your host's settings or web UI, if it has per-device permissions (e.g. Apollo or Rubylight). On hosts that support it (currently Rubylight), choose **Open host administration** from an online PC's menu and sign in with the web UI's account. Listing apps, viewing, launching, each input type, clipboard directions and commands have separate permissions. See [pairing](pairing.md#per-device-permissions).

## Does Disconnect close my game?

Disconnect ends the local stream and leaves the host app running. Select it again to resume. Quit app requests host application exit and needs permission and confirmation.

## Why did changing a preset do nothing?

A PC's own values override the preset for that PC. In that PC's Stream settings, set them to Same as all PCs and save, then start a new stream. [Settings](settings.md) lists every preset write and reset scope.

## Which codec or upscaler should I choose?

Begin with Automatic codec and upscaling Off. Compare measured decode time and image quality on your device before changing them. [Video and latency](video-and-latency.md) explains the choices, HDR and VRR.

## Is the overlay latency the delay I feel?

Statistics shows network round-trip time, available host processing, decode time and display wait separately. Debug adds detailed stage timings. These values do not measure physical input sampling through display scanout; physical end-to-end latency needs an external measurement. Choose the view through **stream menu → Overlay**.

## Can I use my launcher?

Nightjar exports game files for ES-DE and accepts Android intents from Daijisho, Pegasus and other frontends. See [frontends](FRONTENDS.md).

## Does an update preserve pairing?

An update signed with the same key normally preserves the installation. Uninstalling, clearing app storage or restoring onto another device requires fresh pairing. A differently signed APK cannot update the installed app.

## Does Nightjar upload diagnostics automatically?

No. Problem reports are local until you choose a receiving app in Android's share sheet. See [report export](TROUBLESHOOTING.md#export-a-problem-report) and [privacy](PRIVACY.md) for network checks, storage and backups.
