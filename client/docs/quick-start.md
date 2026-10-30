# Quick start

[Documentation](index.md)

This page takes you from installation to a running stream. The defaults are chosen for the device, so no tuning is needed for the first stream.

You need:

- A PC running a Moonlight-compatible host, such as [Sunshine](https://github.com/LizardByte/Sunshine), [Apollo](https://github.com/ClassicOldSong/Apollo), [Vibepollo](https://github.com/Nonary/Vibepollo) or [Rubylight](https://github.com/RamazanKara/Rubylight).
- A device for Nightjar: Android 5.0 or newer (phone, tablet, handheld, Android TV or Fire TV), Windows 10 or 11, or x86-64 Linux with glibc 2.35 or newer.
- A network between them. Start on the same local network, with the PC on Ethernet where possible. To play elsewhere, see [Play away from home](remote-setup.md).

## Five steps to your first stream

1. **Prepare the PC.** Start the host, open its web UI and check that the game is in its app list.
2. **Install Nightjar.** Download it from the [latest release](../README.md#download). On Android, open the APK and allow installation from the app that opened it. On Windows, run the setup. On Linux, install the `.deb` or `.rpm`, or run the `.AppImage`.
3. **Find the PC.** Open Nightjar and select the PC it found. If the PC is not listed, choose **Add PC** and enter its local IP address or host name. Use the PC's own address; `127.0.0.1` refers to this device.
4. **Pair.** Enter the PIN that Nightjar shows in the host's web UI. On a Rubylight PC you can instead choose **Add PC → Scan QR code** on Android, or **Paste link** on the desktop, with the code on the host's **Devices** page. If the host grants permissions per device, allow this device to list, view and launch apps, and the input types you use. [Pairing and permissions](pairing.md) covers each method.
5. **Play.** Open the PC's library and select a game. Use a controller, a mouse and keyboard, or on Android turn on **Settings → Controls → Show on-screen controls**.

A new installation streams at the device's native resolution and highest refresh rate, with a bitrate calculated for both and the codec chosen automatically. The first stream at these values shows what the network and the decoder sustain; the [performance overlay](video-and-latency.md#performance-overlay) shows whether the stream keeps up. Values set for a PC, a game or a profile take precedence over the main settings; see [which value a stream uses](settings.md#which-value-a-stream-uses).

The pairing guide shown on first launch can be skipped and reopened from **Pairing help** on the empty PC list. The **?** button opens the built-in help, which works offline.

## Your PC library

Each PC is one row with a single status line. Selecting it opens the library, starts pairing or wakes the PC, depending on its state. Long-press a PC or game, or use its options button, for its actions; everything else about a PC is under **PC settings…**.

The library shows cover art, lists the running game first and otherwise follows the host's app order. Layouts adapt to tablets, foldables, TVs and desktop windows: wide windows show more columns, short landscape windows use shorter artwork, and content stays clear of hinges, system bars and display cutouts. See [app navigation](controls.md#app-navigation) for keys and shortcuts.

## During a stream

Open the stream menu with:

| Client | Touch or keyboard | Controller |
| --- | --- | --- |
| Android | **Back** (or the back gesture), or **Ctrl+Alt+Shift+M** | **Start+Back+LB+RB** |
| Windows and Linux | **Ctrl+Alt+Shift+Tab** | **Start+Back** |

The top row toggles **Keyboard**, **Overlay** and, when the PC offers it, **Microphone**. The **Input**, **Display**, **Quality** and **Host** sections hold the rest; [the stream menu](controls.md#the-stream-menu) describes every item. Which items appear depends on the host and the device's permissions.

- **Disconnect** ends this device's connection and leaves the game running on the PC. Select the same game to resume.
- **Reconnect stream** (desktop: **Reconnect**) restarts the connection to the running game.
- **Quit game**, last in the menu, asks the host to end the game. It needs the corresponding permission and a confirmation.

Hosts such as Apollo, Vibepollo and Rubylight also accept clipboard text and commands configured on the host, both from the stream menu. A PC whose host supports sharing adds clipboard sync, images, files, shared folders and USB devices; see [sharing](sharing.md).

## Next

- Choose [presets and per-PC profiles](settings.md).
- Set up your [controls](controls.md).
- If pairing or the stream fails, start with [troubleshooting](TROUBLESHOOTING.md).
