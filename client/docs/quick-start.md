# Quick start

[Documentation](index.md)

You need:

- A PC running a Moonlight-compatible streaming host, such as [Sunshine](https://github.com/LizardByte/Sunshine), [Apollo](https://github.com/ClassicOldSong/Apollo), [Vibepollo](https://github.com/Nonary/Vibepollo) or [Rubylight](https://github.com/RamazanKara/Rubylight).
- A device for Nightjar: Android 5.0 or newer (phone, tablet, handheld, Android TV or Fire TV), Windows 10 or 11, or a Linux x86-64 system with glibc 2.39 or newer.
- A network between them. Start on the same local network; connect the PC by Ethernet where you can. To play away from home, see [remote play](remote-setup.md).

## Five steps to your first stream

1. **Prepare the PC.** Start the host, open its web UI and check that the game you want is in its app list.
2. **Install Nightjar.** Download it from the [latest release](../README.md#download). On Android, open the APK and allow installation from the app that opened it. On Windows, run the setup; on Linux, install the `.deb` or `.rpm`, or run the `.AppImage`.
3. **Find the PC.** Open Nightjar and select the PC it found. If the PC is not listed, choose **Add PC** and enter its local IP address or host name. Use the PC's address, not `127.0.0.1`, which points to this device.
4. **Pair.** Enter the PIN Nightjar shows in the host's web UI. On a Rubylight PC, **Add PC → Scan QR code** pairs from the code on its **Devices** page instead (desktop: **Paste link**). If the host sets per-device permissions, allow this device to list, view and launch apps, and the input types you use. See [pairing](pairing.md).
5. **Stream.** Open the PC's library and select a game. Connect a controller, use a mouse and keyboard, or on Android turn on **Settings → Controls → Show on-screen controls**.

New installations stream at this device's native resolution and maximum refresh rate, with a bitrate calculated for both and automatic codec selection. A first stream at these values shows what the network and decoder sustain; the [performance overlay](video-and-latency.md#performance-overlay) shows whether it keeps up. PC, game and profile values take priority over the main settings ([which value a stream uses](settings.md#which-value-a-stream-uses)).

The first-launch pairing guide can be skipped, and is available later from **Pairing help** on the empty PC list. The **?** button opens the built-in help, which works offline.

## Your PC library

Each PC is one row with one status line; tapping it opens the library, pairs or wakes the PC. Long-press a PC or game, or use its options button, for its actions; everything else about a PC is in **PC settings…**. The library shows covers with the running game first and follows the host's app order, uses app UUIDs for shortcuts and refreshes artwork when the host changes its artwork version.

Tablet, foldable, TV and desktop layouts adapt to the current window, preserving library focus on resize and keeping content clear of separating hinges, system bars and cutouts. Wide windows show more columns; short landscape windows use shorter artwork. See [navigation and shortcuts](controls.md#app-navigation).

## During a stream

On Android, press **Back** (or swipe back), **Ctrl+Alt+Shift+M** on a keyboard, or **Start+Back+LB+RB** on a controller to open the stream menu; the first stream shows this once. On Windows and Linux, press **Ctrl+Alt+Shift+Tab** or **Start+Back** on a controller. The top row toggles **Keyboard**, **Overlay** and, when the PC offers it, **Microphone**. Below it, **Input**, **Display**, **Quality** and **Host** hold the rest; see [the stream menu](controls.md#the-stream-menu). Host support and permissions decide which items appear.

**Disconnect** ends this device's connection and leaves the PC app running. Select the same app to resume. **Quit game**, last in the menu, asks the host to end the game and requires permission and confirmation.

Clipboard transfer and host-configured commands need a host that supports them, such as Apollo, Vibepollo or Rubylight. Clipboard transfer from the menu is plain text, explicitly initiated in the foreground, and needs an active session plus the corresponding host permission. A Rubylight PC adds clipboard sync, images, files, shared folders and USB devices; see [sharing](sharing.md). Host commands use the host's configured commands and encrypted server-command transport. Device permissions and host frame-limiter status are under **Host**; **Reconnect stream** sits with Disconnect at the bottom.

## Next

Use [presets and PC profiles](settings.md), set up your [controls](controls.md), or read the [symptom-based fixes](TROUBLESHOOTING.md) if pairing or the stream fails.
