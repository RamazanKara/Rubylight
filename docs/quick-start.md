# Quick start

[Documentation](index.md)

You need an Android device running Android 5.0 or newer, a Moonlight-compatible streaming host running on your PC, and a network connecting them. Start on the same local network. Connect the PC by Ethernet when possible.

## Five steps to your first stream

1. **Prepare the PC.** Start your PC's streaming host, such as [Apollo](https://github.com/ClassicOldSong/Apollo), [Rubylight](https://github.com/RamazanKara/Rubylight), [Sunshine](https://github.com/LizardByte/Sunshine) or [Vibepollo](https://github.com/Nonary/Vibepollo), open its settings or web UI and make sure the app you want appears in its library.
2. **Install Nightjar on Android.** Get an APK from the [download page](../README.md#download), open it and allow installation from your browser or file manager when prompted.
3. **Find the PC.** Open Nightjar and select the discovered PC. If it is absent, choose **Add PC** and enter the PC's local IP address or hostname. Use the PC address, not `127.0.0.1`, which points back to Android.
4. **Pair.** Enter Android's displayed PIN in your host's settings or web UI. If your host has per-device permissions, enable the device and grant application listing, viewing, launching and the input types you intend to use. See [pairing](pairing.md).
5. **Stream.** Open the PC's library and tap an app. Connect a gamepad, use a mouse and keyboard, or enable **Settings → Controls → Show on-screen controls**.

The fresh global video setup is 1280 × 720 at 60 FPS, 10 Mbps and Automatic codec selection. Try a first stream with these values before raising resolution or frame rate. A saved PC profile takes priority over the global setup.

The skippable first-launch pairing guide is also available from the **?** button. The guide screenshots show an example address; enter your own PC's address.

## Your PC library

Material 3 PC cards show connection status, with **Add PC** and grouped action sheets. Long-press a PC or app for its actions. The library follows the host's app order, uses app UUIDs for shortcuts and refreshes artwork when the host changes its artwork version.

Tablet, foldable, TV and desktop layouts adapt to the current window, preserving library focus on resize and keeping content clear of separating hinges, system bars and cutouts. Wide windows show more columns; short landscape windows use shorter artwork. See [navigation and shortcuts](controls.md#app-navigation).

## During a stream

Press Android **Back** or **Ctrl+Alt+Shift+M** to open the menu. Use **Keyboard**, **Input mode**, **Overlay**, **Bitrate**, **Clipboard** or **Host commands** as needed. Host support and permissions determine which actions are available.

**Disconnect** ends this device's connection and leaves the PC app running. Select the same app to resume. **Quit app** asks the host to end the app and requires permission and confirmation.

Clipboard transfer and host-configured commands need a host that supports them (e.g. Apollo or Rubylight). Clipboard transfer is plain text, explicitly initiated in the foreground, and needs an active session plus the corresponding host permission. Host commands use the host's configured commands and encrypted server-command transport. Reconnect, device permissions and host frame-limiter status are under **Host commands**.

## Next

Use [presets and PC profiles](settings.md), set up your [controls](controls.md), or read the [symptom-based fixes](TROUBLESHOOTING.md) if pairing or the stream fails.
