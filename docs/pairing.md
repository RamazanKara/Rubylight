# Pairing and host permissions

[Documentation](index.md)

Pair each Android installation with your PC's streaming host before opening its library. The host recognizes the installation's certificate and device identity.

## Find the PC

Keep the host running and awake on the same network. Select its discovered card, or choose **Add PC** and enter a hostname, IPv4 address or IPv6 address. A custom HTTP port can be supplied as `hostname:port` or `[IPv6-address]:port`; this is the streaming service's HTTP port, not the port of the host's web UI.

On guest networks, VLANs or VPNs, add the PC manually by its address. Manual entry works over any route that reaches the PC through its firewall.

## PIN flow

1. Select the unpaired PC in Nightjar.
2. Keep the displayed PIN dialog open on Android.
3. Enter that PIN in your host's settings or web UI on the PC.
4. Wait for Android to report success, then open the PC's library.
5. If your host has per-device permissions, enable this device and set its permissions there.

Cancel a pairing attempt before starting another. If a PIN is rejected, retry with a fresh PIN and finish or cancel any pending request in the host's web UI. A host that refuses pairing during an active game must finish that session first.

## Host-generated one-time PIN

This needs a host that supports one-time PINs (e.g. Apollo or Rubylight). Create a one-time PIN and passphrase in the host's settings or web UI. On Android, long-press the PC and choose **Pair with one-time PIN**. Enter the four-digit PIN and its passphrase, then pair. The PIN expires after three minutes; the passphrase must contain at least four characters.

This reverses the ordinary flow: you enter the host's PIN on Android. Generate a new one after expiry.

## Per-device permissions

Hosts with per-device permissions (e.g. Apollo or Rubylight) set them in the host's settings or web UI, separately for each paired installation. Android shows the permissions reported by the host and checks them before protected actions. The host stays in charge of every permission. Hosts that do not report permissions keep their usual behavior.

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

Grant list, view, launch and the relevant input permissions for normal play. A view-only device can display video without controlling it. **Remote Monitor** is a view-only role; **Input-only** sends controls without starting video/audio decoders. These entries appear only on hosts that list them (Rubylight and Vibepollo; Apollo lists a Remote Input entry when its input-only mode is on). On other hosts, [Input-only on any PC](controls.md#input-only-on-any-pc) offers Input-only over a small stream; it needs view and input permission.

During a stream, **stream menu → Host → Permissions and frame limits** provides host status and device-permission information. Host application restrictions can further limit an action.

## Identity, updates and removal

Same-install updates keep pairing. Android backups and device transfer exclude the client certificate, private key and installation identity; a restored installation must pair again.

Remove an unwanted PC from Android to clear its saved-host entry, per-PC settings, cached library/artwork and hidden-app choices. Other PCs and the installation's pairing identity are retained. Revoke the device in the host's settings or web UI to remove its host access. Clearing app storage or uninstalling removes the local pairing identity. Launcher entries exported to a folder you selected are separate files and are not removed when deleting a PC.

## Wake a sleeping PC

Pair once while the PC is awake so Nightjar can learn its network adapter's MAC address. Enable Wake-on-LAN in the PC firmware and adapter settings, then tap the offline PC, or choose **Wake PC** from its menu. Waking from sleep, shutdown or another subnet depends on the PC and network. A sent wake packet is not a wake confirmation.
