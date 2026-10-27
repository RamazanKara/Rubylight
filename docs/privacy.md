# Privacy

Rubylight runs on your PC and talks to the devices you pair with it. It has no accounts, analytics, advertising, crash reporting or telemetry upload. It never sends your logs, settings, crash dumps, paired devices or usage to the Rubylight project or to anyone else, and nobody operates a server that your streams pass through.

This page lists everything Rubylight sends over a network, so you can turn off what you don't want. It describes Rubylight 2.3.1 and later, including 3.0; the [code signing policy](code-signing-policy.md) links here.

## Third-party services

Four features contact services outside your network. Only the update check is on by default.

| Feature | When it runs | Where it connects | What that service receives |
| --- | --- | --- | --- |
| Update check | On by default: when the host starts and then once a day, never during a stream, and when you click **Check for updates**. Set `update_check_interval = 0` to turn off the automatic checks ([Updates](configuration.md#updates)). | `api.github.com` (GitHub) | A request for Rubylight's release list with the user agent `Rubylight`, and your IP address. Not your version, host ID, settings or anything about your PC; the comparison with your version happens locally. |
| Update download | When you choose **Install** for an offered update, or automatically if you turn on `auto_update` (off by default). | `github.com` and GitHub's download servers | The download of the installer, and your IP address. |
| Steam covers | Only if the Steam library is on (`steam_enabled`, off by default), for Steam games whose cover is missing or too small. | `shared.fastly.steamstatic.com` (Valve's image servers) | The Steam app IDs of those games, and your IP address. |
| Cover search | When you click **Find cover** for an app in the console. Your browser runs the search; the host then downloads the cover you pick. | `raw.githubusercontent.com` (LizardByte's GameDB on GitHub) and `images.igdb.com` (IGDB) | The first letters of the search and the IDs of matching games, the cover you pick, your IP address and browser user agent, and the console's address as the browser's origin (for example `https://192.168.1.20:47990`). |

Their privacy policies: [GitHub](https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement), [Valve](https://store.steampowered.com/privacy_agreement/), [IGDB, run by Twitch](https://www.twitch.tv/p/legal/privacy-policy/). Uploading your own cover image sends it to your host only.

## Your own devices

Everything else stays between your PC and devices on your network or your own VPN.

- **Discovery.** With `enable_discovery` on (the default), the host announces itself on your local network over mDNS with its name, addresses and port, so Moonlight lists it. Any device that can reach the host can read its name, host ID, local address, ports, version and codec support, which Moonlight needs before pairing.
- **Pairing.** Moonlight and the host exchange a PIN challenge and certificates directly. The host keeps each paired device's certificate and name on your PC.
- **Streaming.** The host sends the picture and sound of your PC to the paired device that started the stream, and receives its keyboard, mouse, touch, pen and controller input. It sends rumble and haptics back. It receives the device's microphone when the device sends it (`stream_mic`), and exchanges clipboard text with devices that have the clipboard permission; the first device you pair gets every permission except host control, later ones can only view until you allow more. Nothing is relayed through a Rubylight server.
- **Native API.** A paired device on the streaming HTTPS port can read your game library, covers and session state, and with host control (off by default) close games and change Radeon settings on the PC. It authenticates with its pairing certificate and talks only to your host ([Native API](native-api.md)).
- **Wake-on-LAN.** A paired device with List, View or Launch can read the MAC addresses and broadcast addresses of the PC's physical network adapters, and their wake settings, so it can wake the PC from sleep. It also sees whether sleep after idle is on and what holds it back, which can say that someone used the PC's keyboard or mouse or the console recently. Moonlight already receives the MAC of the adapter it reached.
- **Hardware readings.** With `hardware_metrics` on (off by default), a paired device that is streaming can ask for CPU and GPU readings for its overlay. They go only to that device ([Hardware metrics](hardware-metrics.md)).
- **Your router.** With `upnp` on (off by default), the host asks your router to forward its streaming ports. The router receives the host's local address, the ports and a description that includes the host ID.

The installer makes no internet connections. Support bundles and crash dumps are files on your PC that you choose whether to share.

## Code signing

Signed Windows releases are signed through SignPath. That service receives the release build from GitHub Actions, never anything from your PC. See the [code signing policy](code-signing-policy.md).

## Questions

Open an [issue](https://github.com/RamazanKara/Rubylight/issues) if something here is unclear or wrong.
