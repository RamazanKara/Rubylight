# Quick start

[Documentation](index.md)

You need an Android device running Android 5.0 or newer, the Rubylight host running on your PC, and a network connecting them. Start on the same local network. Connect the PC by Ethernet when possible.

## Five steps to your first stream

1. **Prepare the PC.** Start the [Rubylight host](https://github.com/RamazanKara/Rubylight), open its web console and make sure the app you want appears in its library.
2. **Install Rubylight on Android.** Get an APK from the [download page](../README.md#download), open it and allow installation from your browser or file manager when prompted.
3. **Find the PC.** Open Rubylight and select the discovered PC. If it is absent, choose **Add PC** and enter the PC's local IP address or hostname. Use the PC address, not `127.0.0.1`, which points back to Android.
4. **Pair.** Enter Android's displayed PIN in **Devices** in the host web console. Enable the device and grant application listing, viewing, launching and the input types you intend to use. See [pairing](pairing.md).
5. **Stream.** Open the PC's library and tap an app. Connect a gamepad, use a mouse and keyboard, or enable **Settings → Controls → Show on-screen controls**.

The fresh global video setup is 1280 × 720 at 60 FPS, 10 Mbps and Automatic codec selection. Try a first stream with these values before raising resolution or frame rate. A saved PC profile takes priority over the global setup.

The first-launch pairing guide is also available from the **?** button. Use your PC's real address when adding it manually.

## Your PC library

PC cards show whether each host is online, offline or unpaired. Long-press a PC for pairing, connection tests, stream settings and Wake-on-LAN. Long-press a game for its actions, including launcher shortcuts. The library follows the host's app order and adapts to your screen. See [navigation and shortcuts](controls.md#app-navigation).

## During a stream

Press Android **Back** or **Ctrl+Alt+Shift+M** to open the menu. Use **Keyboard**, **Input mode**, **Overlay**, **Bitrate**, **Clipboard** or **Host commands** as needed. Host permissions determine which actions are available.

**Disconnect** ends this device's connection and leaves the PC app running. Select the same app to resume. **Quit app** asks the host to end the app and requires permission and confirmation.

Clipboard transfer is plain text, explicitly initiated in the foreground, and needs an active session plus the corresponding host permission. Host commands use the host's configured commands and encrypted server-command transport; sending a command does not confirm its execution. Reconnect, device permissions and host frame-limiter status are under **Host commands**.

## Next

Use [presets and PC profiles](settings.md), set up your [controls](controls.md), or read the [symptom-based fixes](troubleshooting.md) if pairing or the stream fails.
