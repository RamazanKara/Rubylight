# FAQ

[Documentation](index.md)

## What do I need?

An Android 5.0+ device, a PC running the Rubylight host and a working network between them. Start with the [quick start](quick-start.md). Individual HDR, USB and Vulkan features have additional device requirements.

## Does Rubylight run the game on Android?

The game runs on your PC. Rubylight streams its video/audio to Android and sends your controls back.

## Does it need root?

The normal `nonRoot` build does not. A separate root flavor exists for older Android mouse-capture use; see [building](building.md#flavors-and-outputs).

## Why do filenames or package names contain a different identifier?

The product is Rubylight and the repository is `rubylight-android`. The Android package ID remains `com.butterpollo.client` so existing installations keep their identity. Exported folders, report filenames and CI artifacts also use fixed identifiers; use the exact names shown in these guides.

## Can I stream outside my home network?

Yes, when your Android device can reach the host's streaming services. Establish local pairing first, then configure a routed connection such as your own VPN. Discovery may not cross that route, so add the reachable host address manually. Network and host policy determine access.

## Where do I set device permissions?

In **Devices** in the Rubylight host web console. Listing apps, viewing, launching, each input type, clipboard directions and commands have separate permissions. See [pairing](pairing.md#per-device-permissions).

## Does Disconnect close my game?

Disconnect ends the local stream and leaves the host app running. Select it again to resume. Quit app requests host application exit and needs permission and confirmation.

## Why did changing a preset do nothing?

A saved PC profile can override the preset's global video values. Enable Use global settings in that PC's Stream settings and save, then start a new stream. [Settings](settings.md) lists every preset write and reset scope.

## Which codec or upscaler should I choose?

Begin with Automatic codec and upscaling Off. Compare measured decode time and image quality on your device before changing them. [Video and latency](video-and-latency.md) explains the choices, HDR and VRR.

## Is the overlay latency the delay I feel?

The Compact value is network round-trip time plus completed decode time. It excludes other stages, including physical input sampling and display scanout. Advanced stats separate the available stages; physical end-to-end latency needs an external measurement.

## Can I use my launcher?

Rubylight exports game files for ES-DE and accepts Android intents from Daijisho, Pegasus and other frontends. See [frontends](frontends.md).

## Does an update preserve pairing?

An update signed with the same key normally preserves the installation. Uninstalling, clearing app storage or restoring onto another device requires fresh pairing. A differently signed APK cannot update the installed app.

## Does Rubylight upload diagnostics automatically?

No. Problem reports are local until you choose a receiving app in Android's share sheet. See [report export](troubleshooting.md#export-a-problem-report) and [privacy](PRIVACY.md) for network checks, storage and backups.
