# Privacy

[Documentation](README.md) · [Code signing policy](code-signing-policy.md)

Rubylight runs on your PC and communicates with the devices you pair with it. It has no accounts, analytics, advertising, crash reporting or telemetry upload. It never sends your logs, settings, crash dumps, paired devices or usage to the Rubylight project or to anyone else, and no server operated by the project carries your streams.

This page lists everything Rubylight sends over a network, so you can turn off what you do not want. It applies to Rubylight 2.3.1 and later, including 3.0. The [code signing policy](code-signing-policy.md) links here.

## Third-party services

Six features contact services outside your network. The update check and Steam covers are on by default; the others run only when you use them.

| Feature | When it runs | Where it connects | What that service receives |
| --- | --- | --- | --- |
| Update check | On by default: when the host starts and then once a day, never during a stream, and when you select **Check for updates**. Set `update_check_interval = 0` to turn off the automatic checks ([Updates](configuration.md#updates)). | `api.github.com` (GitHub) | A request for Rubylight's release list with the user agent `Rubylight`, and your IP address. Not your version, host ID, settings or anything about your PC; the comparison with your version happens locally. |
| Update download | When you choose **Install** for an offered update, or automatically if you turn on `auto_update` (off by default). | `github.com` and GitHub's download servers | The download request for the installer, and your IP address. |
| Steam covers | While the Steam library is on (`steam_enabled`, on by default), for Steam games whose cover is missing from Steam's local cache or too small. Turn the Steam library off to stop it. | `shared.fastly.steamstatic.com` (Valve's image servers) | The Steam app IDs of those games, and your IP address. |
| Cover search | When you select **Find cover** for an app in the console. Your browser runs the search; the host then downloads the cover you pick. | `raw.githubusercontent.com` (LizardByte's GameDB on GitHub) and `images.igdb.com` (IGDB) | The first letters of the search and the IDs of matching games, the cover you pick, your IP address and browser user agent, and the console's address as the browser's origin (for example `https://192.168.1.20:47990`). |
| Nucleus Co-op | When you select **Set up co-op** on a game's page, or on the first co-op or background stream, unless `nucleus_path` points to your own copy. Downloaded once. | `github.com` and GitHub's download servers | The download request for Nucleus Co-op 2.4.2 (`NucleusApp.zip`, 97 MB) from SplitScreen.Me's release, and your IP address ([Co-op and background streaming](nucleus-coop.md#installing-nucleus-co-op)). |
| Neural rendering | When you turn on **Neural Rendering** for a game and the mod's pinned release has not been downloaded yet. Off for every game by default. | `api.github.com` (GitHub) for the release's file list by its tag, then `github.com` and GitHub's download servers for parts of the archives (their file lists) and the whole release, about 1 GB | Requests for that release and its files with the user agent `Rubylight`, and your IP address. Not your game, host ID, settings or anything about your PC. |

Their privacy policies: [GitHub](https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement), [Valve](https://store.steampowered.com/privacy_agreement/), [IGDB, operated by Twitch](https://www.twitch.tv/p/legal/privacy-policy/). A cover image you upload yourself goes to your host only.

## Your own devices

Everything else stays between your PC and devices on your network or your own VPN.

- **Discovery.** With `enable_discovery` on (the default), the host announces itself on your local network over mDNS with its name, addresses and port, so Moonlight lists it. Any device that can reach the host can read its name, host ID, local address, ports, version and codec support, which Moonlight needs before pairing.
- **Pairing.** The client and the host exchange a PIN challenge and certificates directly. The host stores each paired device's certificate and name on your PC.
- **Streaming.** The host sends your PC's picture and sound to the paired device that started the stream, and receives its keyboard, mouse, touch, pen and controller input; it sends rumble and haptics back. It receives the device's microphone when the device sends it (`stream_mic`), and exchanges clipboard contents with devices that have the clipboard permission. The first device you pair gets every permission except host control; later devices get what you choose when you pair them, **Play games** by default. Nothing is relayed through a Rubylight server.
- **Native API.** A paired device on the streaming HTTPS port can read your game library, covers and session state, and with host control (off by default) close games and change Radeon settings on the PC. It authenticates with its pairing certificate and talks only to your host ([Native API](native-api.md)).
- **Wake-on-LAN.** A paired device with List, View or Launch can read the MAC addresses and broadcast addresses of the PC's physical network adapters, and their wake settings, so it can wake the PC from sleep. It also sees whether sleep after idle is on and what is holding it back, which can reveal that someone recently used the PC's keyboard or mouse or the console. Moonlight already receives the MAC address of the adapter it reached.
- **Hardware readings.** With `hardware_metrics` on (off by default), a paired device that is streaming can request CPU and GPU readings for its overlay. They go only to that device ([Hardware metrics](hardware-metrics.md)).
- **Webhooks.** With hooks configured (none by default), the host posts each event you selected to the URL you entered: the event, the device's name and ID, the app's name and ID, the session ID and the host's name and ID, signed with the hook's secret ([Webhooks](webhooks.md)).
- **Clipboard.** A device with **Read the clipboard** receives what you copy on the PC: text and, while `clipboard_image` is on (the default), images. A device with **Write to the clipboard** puts what it copies on the PC's clipboard. Pairing grants neither permission. Background and co-op streams never touch the PC user's clipboard.
- **Files and shared folders.** With `file_transfer` on (off by default), a device with **Upload files** or **Download files** writes to and reads from one folder, the signed-in user's `Downloads\Rubylight` unless you choose another. A device sees a shared folder only after you share it with that device, read-only or read-write. Files are read and written as the signed-in Windows user, only inside those folders, and links inside them are not followed ([Configuration](configuration.md#clipboard-files-and-usb)).
- **USB devices.** With `usb_forwarding` on (off by default), a device with **USB devices** can attach a USB device plugged into it to this PC during a stream, where Windows uses it like a locally connected device. The traffic travels inside the stream's TLS connection; no USB/IP port is open to the network ([USB forwarding](usb-forwarding.md)).
- **Microphone.** The device's microphone reaches the PC only while the device sends it, which you enable in the client, and while `stream_mic` is on. It plays into Steam Streaming Microphone and is not recorded by the host ([Microphone](microphone.md)).
- **Pairing QR code.** With `qr_pairing` on (the default), the console can show a QR code holding a one-time PIN, its passphrase, and the host's address, port, ID, name and certificate fingerprint. Anyone who scans it within 3 minutes can pair once.
- **Your router.** With `upnp` on (off by default), the host asks your router to forward its streaming ports. The router receives the host's local address, the ports and a description that includes the host ID.

The installer makes no internet connections. Support bundles and crash dumps are files on your PC; you decide whether to share them.

## Code signing

Signed Windows releases are signed through SignPath. That service receives the release build from GitHub Actions, never anything from your PC. See the [code signing policy](code-signing-policy.md).

## Questions

Open an [issue](https://github.com/RamazanKara/Rubylight/issues) if anything on this page is unclear or wrong.
