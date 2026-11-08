# Pairing and host permissions

[Documentation](index.md)

Pair each Android installation with the Rubylight host before opening its library. Pairing identifies the device; the host's per-device permissions determine what it can do.

## Find the PC

Keep the host running and awake on the same network. Select its discovered card, or choose **Add PC** and enter a hostname, IPv4 address or IPv6 address. A custom HTTP port can be supplied as `hostname:port` or `[IPv6-address]:port`; this is the streaming service's HTTP port, not the web console's port.

Discovery may not cross guest networks, VLANs or VPNs. Manual entry needs a working route to the PC; it does not bypass a firewall.

## PIN flow

1. Select the unpaired PC in Rubylight.
2. Keep the displayed PIN dialog open on Android.
3. Open the Rubylight host web console on the PC and enter that PIN under **Devices**.
4. In the host's **Devices** page, enable this device and grant the permissions below.
5. Wait for Android to report success, then open the PC's library and select an app.

Cancel a pairing attempt before starting another. If a PIN is rejected, retry with a fresh PIN and finish or cancel any pending request in the host console. A host that refuses pairing during an active game must finish that session first.

## Host-generated one-time PIN

Create a one-time PIN and passphrase on the Rubylight host. On Android, long-press the PC and choose **Pairing → Pair with one-time PIN**. Enter the four-digit PIN and its passphrase, then pair. The PIN expires after three minutes; the passphrase must contain at least four characters.

This reverses the ordinary flow: you enter the host's PIN on Android. Generate a new one after expiry.

## Per-device permissions

Permissions are set on the Rubylight host's **Devices** page, separately for each paired installation. Android shows the permissions reported by the host and checks them before protected actions. It does not grant itself access.

| Permission | What it allows |
| --- | --- |
| List applications | Read the host's permitted library entries. |
| View streams | View an existing host stream. |
| Launch and quit applications | Start or end host apps, subject to host policy and confirmations. |
| Controller input | Send gamepad input. |
| Touch input | Send native multi-touch input. |
| Pen input | Send native pen input. |
| Mouse input | Send a physical or emulated mouse, including trackpad/direct mouse touch modes. |
| Keyboard input | Send hardware or on-screen keyboard input. |
| Send clipboard | Send explicitly selected text from Android to the PC. |
| Read clipboard | Request text from the PC. |
| Server commands | Send a command configured by the host administrator. |

Grant list, view, launch and the relevant input permissions for normal play. A view-only device can display video without controlling it. **Remote Monitor** is a view-only role; **Input-only** sends controls without starting video/audio decoders. These entries depend on what the host exposes.

During a stream, **Host commands** provides host status and device-permission information. A sent command has no execution-result acknowledgement. Host application restrictions can further limit an action.

## Identity, updates and removal

Same-install updates keep pairing. Android backups and device transfer exclude the client certificate, private key and installation identity; a restored installation must pair again.

Remove an unwanted PC from Android and revoke the device in the host console to remove its host access. Clearing app storage or uninstalling removes the local pairing identity.

## Wake a sleeping PC

Pair once while the PC is awake so Rubylight can learn its network adapter's MAC address. Enable Wake-on-LAN in the PC firmware and adapter settings, then long-press the offline PC and choose **Wake PC**. Waking from sleep, shutdown or another subnet depends on the PC and network. A sent wake packet is not a wake confirmation.
