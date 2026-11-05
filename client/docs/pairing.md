# Pairing and host permissions

[Documentation](index.md)

Each Nightjar installation pairs with a PC's host once. Pairing exchanges certificates; afterwards the host recognizes the installation by its client certificate, and Nightjar recognizes the PC by the host's certificate. The steps are the same on Android and on Nightjar for Windows and Linux unless a step names one of them.

| Method | Host support | Who enters what |
| --- | --- | --- |
| [PIN](#pin-flow) | Every Moonlight-compatible host | Nightjar shows a PIN; you enter it on the host |
| [One-time PIN](#host-generated-one-time-pin) | Hosts with one-time PINs, such as Apollo, Vibepollo and Rubylight | The host shows a PIN and passphrase; you enter them in Nightjar |
| [QR code or pairing link](#pair-with-a-qr-code) | Hosts with QR pairing (Rubylight 3.0) | You scan the host's code, or open or paste its link |
| [From a paired device](#approve-from-a-paired-device) | Rubylight with `pairing.approve` | The new device shows a PIN; you enter it on a paired device with Host control |

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

## Approve from a paired device

A TV or console can pair without a walk to the PC. Select the PC on the new device so it shows its PIN, then on a device that is already paired:

1. Home shows **"Living room TV" wants to pair**, or open the PC's options and choose **Pair another device**. The list says what to do on the new device when nothing waits.
2. Select the request, compare the address with the new device, and enter its PIN. A controller works: every digit is a button, and the digit keys of a keyboard work too.
3. Choose **Approve**, or **Deny** to end the request. Backing out leaves it waiting.

The new device gets what a PIN entered on the PC gives by default, Play, and never more than the approving device holds. Host control and shared folders are not passed on. Three wrong PINs end the requests from that device for five minutes; the PC's own Devices page can still pair it. The PC shows a toast for every approval, and logs and sends a webhook naming both devices.

This needs **Host control** on the approving device. No device holds it until it is turned on in the host's web UI (**Devices** → device → **Other**), the first paired one included; without it, Home shows no requests and the options don't list the entry.

## Host-generated one-time PIN

Create a one-time PIN and passphrase in the host's web UI. In Nightjar, open the PC's options (on Android, long-press the PC) and choose **Pair with one-time PIN**. Enter the four-digit PIN and the passphrase, which has at least four characters. The PIN expires after three minutes; generate a new one after that.

## Pair with a QR code

A host with QR pairing (Rubylight 3.0) shows a QR code on its **Devices** page for each of the PC's network addresses. The code carries a one-time PIN that works once and expires after three minutes.

- **Android:** choose **Add PC**, then **Scan QR code**, and point the camera at the code. Nightjar asks for camera access only at this point; without it, enter the address instead. Devices without a camera do not show the button.
- **Windows and Linux:** choose **Copy pairing link** under the code on the PC, then **Paste link** in Nightjar's **Add PC** dialog, or paste the link into its address field.
- **Any device with Nightjar:** choose **Open in Nightjar** under the code, or scan the code with another QR reader and open the link. See [Open a pairing link](#open-a-pairing-link).

Scanned in Nightjar or pasted there, Nightjar adds the PC at the code's address and pairs immediately. It refuses to pair if the PC presents a certificate other than the one named in the code, so a code only pairs with the PC that showed it. A code is consumed by a failed attempt as well as a successful one; scanning a used or expired code again reports that and asks for a new code.

### Open a pairing link

Android, and the Windows installer and the Linux packages (including the Flatpak), make Nightjar the handler of `rubylight:` links. The portable Windows zip and a Linux archive or AppImage that was not integrated into the desktop do not register it; paste the link there.

Any web page can open a link, so a link opened from outside Nightjar never pairs on its own. Nightjar first asks, naming the PC and its address, with **Pair** and **Cancel**; focus starts on **Cancel**, and a controller reaches both. **Pair** stays off for a second after the question appears or changes, so a click meant for something else cannot land on it. Nothing is added or paired before **Pair**, and the certificate check above still applies. A name in a link is the link's claim, so check the address. A link that is not valid, including one of another version or a very long one, is refused like a pasted one.

A link cannot move a PC Nightjar knows. If a PC paired here has a certificate other than the one the link names, a link to its address is refused, and so is an address that answers with its identity. A PC's own link still finds it at a new address.

On Android, the question opens in its own window, and it leaves no entry in Recents. A link that arrives during a stream or a pairing waits for it to end, and only the newest link waits.

On Windows and Linux, a link goes to the window that is already open, and that window comes to the front. It waits for a running stream or an open dialog to end, and only the newest link waits. With no window open, the link opens one. Nightjar otherwise starts as usual, and more than one window can be open; the first one takes the links.

### Pairing link

The QR code contains a single link; **Copy pairing link** copies the same text and **Open in Nightjar** opens it:

```
rubylight://pair?v=1&host=<address>&port=<http port>&otp=<one-time PIN>&pass=<passphrase>&id=<host uniqueid>&fp=<certificate SHA-256>&name=<host name>
```

- `v` is `1`; other versions are refused.
- `host`, `port`, `otp` and `fp` are required. `host` is a host name or IP address of up to 255 ASCII characters: letters, digits, `.`, `-` and `_`, or an IPv6 address, in brackets or not, whose zone alone may follow a `%`. `port` is the host's HTTP port, 1 to 65535. `fp` is the SHA-256 of the host certificate's DER encoding, as 64 hexadecimal digits.
- `pass`, `id` and `name` are optional. Unknown keys are ignored; a repeated key invalidates the link.
- Values are percent-encoded UTF-8. A link longer than 4096 bytes is refused.

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
