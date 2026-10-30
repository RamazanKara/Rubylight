# Pairing and host permissions

[Documentation](index.md)

Each Nightjar installation pairs with a PC's host once. Pairing exchanges certificates; afterwards the host recognizes the installation by its client certificate, and Nightjar recognizes the PC by the host's certificate. The steps are the same on Android and on Nightjar for Windows and Linux unless a step names one of them.

| Method | Host support | Who enters what |
| --- | --- | --- |
| [PIN](#pin-flow) | Every Moonlight-compatible host | Nightjar shows a PIN; you enter it on the host |
| [One-time PIN](#host-generated-one-time-pin) | Hosts with one-time PINs, such as Apollo, Vibepollo and Rubylight | The host shows a PIN and passphrase; you enter them in Nightjar |
| [QR code or pairing link](#pair-with-a-qr-code) | Hosts with QR pairing (Rubylight 3.0) | You scan the host's code, or paste its link |

## Find the PC

Keep the host running and the PC awake on the same network. Select the PC Nightjar discovered, or choose **Add PC** and enter a host name, IPv4 address or IPv6 address. A custom port can be given as `hostname:port` or `[IPv6-address]:port`; it is the streaming service's HTTP port, not the port of the host's web UI.

Discovery does not cross guest networks, VLANs or VPNs. Add the PC by its address there; manual entry works over any route that reaches the PC through its firewall.

## PIN flow

1. Select the unpaired PC in Nightjar.
2. Keep the PIN dialog open.
3. Enter the PIN in the host's web UI or settings on the PC.
4. Wait for Nightjar to report success, then open the PC's library.
5. If the host grants permissions per device, enable this device and set its permissions there.

Cancel a pairing attempt before starting another. If the host rejects a PIN, finish or cancel any pending request in the host's web UI and try again with a new PIN. A host that refuses pairing while a game is streaming must end that session first.

## Host-generated one-time PIN

Create a one-time PIN and passphrase in the host's web UI. In Nightjar, open the PC's options (on Android, long-press the PC) and choose **Pair with one-time PIN**. Enter the four-digit PIN and the passphrase, which has at least four characters. The PIN expires after three minutes; generate a new one after that.

## Pair with a QR code

A host with QR pairing (Rubylight 3.0) shows a QR code on its **Devices** page for each of the PC's network addresses. The code carries a one-time PIN that works once and expires after three minutes.

- **Android:** choose **Add PC**, then **Scan QR code**, and point the camera at the code. Nightjar asks for camera access only at this point; without it, enter the address instead. Devices without a camera do not show the button.
- **Windows and Linux:** choose **Copy pairing link** under the code on the PC, then **Paste link** in Nightjar's **Add PC** dialog, or paste the link into its address field.

Nightjar adds the PC at the code's address and pairs immediately. It refuses to pair if the PC presents a certificate other than the one named in the code, so a code only pairs with the PC that showed it. A code is consumed by a failed attempt as well as a successful one; scanning a used or expired code again reports that and asks for a new code.

### Pairing link

The QR code contains a single link; **Copy pairing link** copies the same text:

```
rubylight://pair?v=1&host=<address>&port=<http port>&otp=<one-time PIN>&pass=<passphrase>&id=<host uniqueid>&fp=<certificate SHA-256>&name=<host name>
```

- `v` is `1`; other versions are refused.
- `host`, `port`, `otp` and `fp` are required. `port` is the host's HTTP port, 1 to 65535. `fp` is the SHA-256 of the host certificate's DER encoding, as 64 hexadecimal digits.
- `pass`, `id` and `name` are optional. Unknown keys are ignored; a repeated key invalidates the link.
- Values are percent-encoded UTF-8.

The PIN and passphrase pair exactly like a host-generated one-time PIN. Treat the link as a secret until it expires: anyone on your network who uses it within those three minutes can pair.

## Per-device permissions

Apollo, Vibepollo, Rubylight and other hosts with per-device permissions set them in the host's web UI, separately for each paired installation. Nightjar shows the permissions the host reports and checks them before protected actions; the host remains the authority for every permission. Hosts that do not report permissions behave as usual.

| Permission | What it allows |
| --- | --- |
| List applications | Read the host's permitted library entries. |
| View streams | View an existing host stream. |
| Launch and quit applications | Start or end host apps, subject to host policy and confirmations. |
| Controller input | Send gamepad input. |
| Touch input | Send native multi-touch input. |
| Pen input | Send native pen input. |
| Mouse input | Send a physical or emulated mouse, including the trackpad and direct touch modes. |
| Keyboard input | Send hardware or on-screen keyboard input. |
| Send clipboard | Send explicitly selected text from this device to the PC. |
| Read clipboard | Request text from the PC. |
| Server commands | Run a command configured by the host's administrator. |

A Rubylight 3.0 host adds permissions for its extensions: **Host control** (closing games and changing Radeon settings, see [the session menu](console.md#session-and-graphics)), **Upload files** and **Download files**, shared folders, USB forwarding, and **Can join co-op games** (see [sharing](sharing.md) and [Join co-op](console.md#join-co-op)). When an action needs a permission the device lacks, Nightjar names it and points to the Devices page of the host's web UI.

For normal play, grant list, view, launch and the input permissions you use. A device with view permission only can watch without controlling.

Hosts that list these roles also offer two restricted ones: **Remote Monitor** watches only, and **Input-only** sends controls without starting the video and audio decoders. Rubylight and Vibepollo list both; Apollo lists a Remote Input entry when its input-only mode is on. On other hosts, [Input-only on any PC](controls.md#input-only-on-any-pc) provides the same through a small stream and needs view and input permissions.

During a stream, **stream menu → Host → Permissions and frame limits** shows the host's status and this device's permissions. The host's per-app restrictions can limit an action further.

## Identity, updates and removal

Updates of the same installation keep its pairing. Android backups and device transfers exclude the client certificate, private key and installation identity, so a restored installation pairs again.

**Remove PC** clears the PC's saved entry, per-PC settings, cached library and artwork, and hidden-app choices. Other PCs and this installation's identity are unaffected. To revoke the device's access, remove it in the host's web UI. Clearing app storage or uninstalling removes the installation's identity. Launcher entries exported to a folder are separate files and remain after a PC is removed.

## Wake a sleeping PC

Pair once while the PC is awake, so Nightjar learns the MAC address of its network adapter. Enable Wake-on-LAN in the PC's firmware and adapter settings. Then select the offline PC, or choose **Wake PC** from its menu (desktop: **Wake up**).

Whether a PC wakes from sleep, from shutdown or from another subnet depends on the PC and the network. A sent wake packet does not confirm that the PC woke; the PC is ready when Nightjar shows it online.
