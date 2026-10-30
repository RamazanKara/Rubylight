# Getting started

[Documentation](README.md) · [Configuration](configuration.md) · [Troubleshooting](troubleshooting.md)

Rubylight runs on the Windows PC that has the games; a Moonlight-compatible client runs on the device you play from. This guide installs the host, pairs one device and starts a desktop stream. Get a 1080p60 SDR stream working first, then add HDR, higher frame rates or other devices.

**On this page:** [Before you start](#before-you-start) · [Install](#install-or-run-portable) · [Open the console](#open-the-console) · [Pair a device](#pair-a-device-and-start-desktop) · [Stream formats](#choose-your-stream-format) · [Displays and updates](#virtual-displays-and-updates) · [Import a profile](#bring-an-existing-profile) · [Upgrading](#upgrading)

## Before you start

| Requirement | Details |
| --- | --- |
| **Host PC** | Windows 10 or 11 x64 with an **AMD Radeon GPU** that has a working AMF video encoder, and AMD Software. The stream formats on offer depend on the GPU's encoder; the [hardware matrix](hardware-matrix.md) lists each generation. Rubylight 3.x does not run on NVIDIA or Intel graphics, and setup stops and says so; those PCs use [Rubylight 2.6.x](https://github.com/RamazanKara/Rubylight/releases/tag/v2.6.0). |
| **Client** | [Nightjar](https://github.com/RamazanKara/nightjar), Moonlight or another Sunshine-compatible client on the device you play from. |
| **Network** | Both devices on the same local network for the first pairing and stream. Close any other streaming host that uses the default ports. |
| **A display to stream** | An active monitor, or the virtual display the installer sets up (Windows 11 23H2 or later). The portable package streams a monitor unless the drivers are installed separately. |

Upgrading an existing host: see [Upgrading](#upgrading). Moving a portable Apollo or Vibepollo profile: read [Bring an existing profile](#bring-an-existing-profile) before the first launch.

## Install or run portable

Download a package from the [3.0.0 release](https://github.com/RamazanKara/Rubylight/releases/tag/v3.0.0):

| Package | How to start | Use |
| --- | --- | --- |
| `rubylight-setup-3.0.1.exe` | Run the installer, then open **Rubylight** from the Start menu. | Normal use: the Windows service, start with Windows, virtual displays and emulated controllers. |
| `rubylight-3.0.1-windows-x64.zip` | Extract the whole archive and open **Start Rubylight.exe**. | Trying the host on a physical display without installing anything. |

The installer sets up the host, the Windows service, Rubylight's virtual display driver and, unless you clear its box, the VHF driver for emulated controllers. **Install USB audio and haptics support** adds DualSense and Steam Deck USB audio and haptics through usbip-win2; it is off by default, can reconnect USB devices and may need a restart ([DualSense USB audio and haptics](dualsense-haptics.md)). A detected Vibepollo installation is migrated with its settings and paired devices.

**The virtual display driver.** Rubylight's driver replaces libvirtualdisplay (SudoVDA), which setup removes once Rubylight's driver is running. Until releases are code-signed, setup signs the driver on this PC with a key it deletes immediately afterwards; Windows needs neither test mode nor a restart. Smart App Control, Windows in S mode and App Control policies refuse such a driver. Setup and the console then say so, nothing is left half-installed, and streams use a physical display.

- **Install folder.** New installations go to `C:\Program Files\Rubylight`; **Change folder…** picks another folder on a local drive. A folder that already holds other files gets a `Rubylight` folder inside it. A folder outside Program Files receives the same permissions as Program Files, so only administrators can change the programs the service runs. Updates install in place. To move an installation, uninstall it (settings and paired devices stay in `%PROGRAMDATA%\Butterpollo`) and install again. An installation in a folder that non-administrators own, such as `D:\Games\Rubylight`, cannot be updated in place; the installer moves it to `C:\Program Files\Rubylight` ([Troubleshooting](troubleshooting.md#an-update-says-rubylight-cannot-be-updated-in-its-folder)).
- **Installing during a stream.** Setup reports a running stream; **Install** ends it, closes games started from a client, installs and starts Rubylight again. Reconnect when setup has finished (16 seconds on the RX 7900 XT test host); the setup window shows the result on the host's desktop. **Help → Updates → Install now** does the same from the console.
- **Unattended installs.** `rubylight-setup-<version>.exe --quiet` refuses to stop a stream unless you add `--end-streams`. `--install-dir <folder>` chooses the folder for a new installation.

The portable package installs no service. Virtual displays and emulated controllers need their Windows drivers, and the bundled virtual display driver is reached through the installed service. Keep the extracted libraries and the `assets` folder next to the executables. If Sunshine, Apollo or Vibepollo is serving the default ports, close it first or run Rubylight with [its own profile and ports](configuration.md#multiple-instances).

Releases up to 2.0.1 also carry a `butterpollo-setup-<version>.exe`, the same installer under the project's former name.

## Open the console

The launcher opens **https://localhost:47990** on the host PC; a custom base port moves the console with it. The console's certificate covers `localhost`, `127.0.0.1` and `::1`, and setup adds its issuer, **Rubylight local console**, to Windows' trusted roots, so browsers on the host PC open it without a warning. That root can vouch only for the loopback names, and its private key is deleted once it has signed the certificate. From another device, by the host's network address, the browser shows a certificate warning; check that the address is your own host's before you continue.

Create the administrator account when asked; the first account must be created on the host PC. After an import from Apollo, Vibepollo or Sunshine, sign in with that host's console name and password. To set a new sign-in, run the host program with `--creds` from an administrator PowerShell on the host PC (the sign-in page shows the full path):

```powershell
& "C:\Program Files\Rubylight\butterpollo.exe" --creds NAME PASSWORD
```

Use a password of at least 8 characters. Browsers that are signed in must sign in again. A host upgraded from Butterpollo keeps its original install folder.

**Home** leads through three steps: pair a device, see your games, play. Its readiness check (**View checks**) covers the video encoder, virtual display, audio and screen capture. An encoder and an active monitor are enough for a first desktop stream; a virtual display showing **Off** is expected when you stream a monitor.

A new installation gives each device its own virtual display on Windows 11 23H2 and later. To stream a monitor instead, set **Settings → Picture → Virtual display** to **Off, stream a monitor**. With a virtual display, **While streaming, your monitors** decides whether the other monitors turn off (the default) or stay on.

Leave **All settings → Streaming → Capture method** on **Automatic**. It uses direct driver push on Rubylight's virtual displays and Windows Graphics Capture on other displays, and falls back to Desktop Duplication where needed. A capture method imported from another host is kept as it was.

## Pair a device and start Desktop

1. Open the client on a device on the same network.
2. Pair it:
   - **Nightjar** scans the QR code on the console's **Home** (until the first device is paired) or **Devices** page. Nightjar on Windows and Linux pastes the pairing link instead.
   - **Moonlight** lists the host; select it, or add it by the PC's local IP address if it is not listed. Moonlight shows a four-digit PIN. On the console's **Devices** page, enter it for the waiting device, choose what the device may do (**Play games** by default) and select **Pair**.

   The QR code and the pairing link work once, for three minutes. When pairing finishes, the device appears under **Paired devices**.
3. Set the client to **1920×1080 at 60 FPS** with HDR off. Use H.264, or HEVC if the device decodes it in hardware.
4. Launch **Desktop**. Check that windows move smoothly, sound plays and input reaches the PC.

Once that works, raise the resolution and frame rate in the client, then turn on HDR if both the client and the streamed display support it. The stream card on **Home** shows what was negotiated.

If the client does not find the host, see [discovery and pairing](troubleshooting.md#moonlight-cannot-find-or-pair-with-the-pc). If Desktop shows a black picture, see [capture and displays](troubleshooting.md#black-picture-no-display-or-wgc-fails). If pairing works but launching is refused, check that the device is enabled and has **List apps**, **View streams** and **Launch apps** under **Devices → Edit**.

Nightjar also shows the game library, a page for each game and the session's controls. To let a device close games or change Radeon settings, turn on **Host control** for it; pairing leaves it off ([Host control](configuration.md#host-control-for-a-device)). The clipboard, file transfer, shared folders and USB devices each need their own permission, which pairing never grants ([Clipboard, files and USB](configuration.md#clipboard-files-and-usb), [Privacy](privacy.md#your-own-devices)).

## Choose your stream format

| Format | Use it when |
| --- | --- |
| H.264 | You want an SDR baseline, or the client has no newer hardware decoder. |
| HEVC | The client decodes HEVC. Its 10-bit profile carries HDR. |
| AV1 | The host's encoder and the client's decoder both support AV1. Its 10-bit profile carries HDR. |
| PyroWave | You want full-resolution colour, including 10-bit HDR 4:4:4, on a fast local network, with Nightjar or another PyroWave-capable client. Connect a PC client by cable. |

H.264, HEVC and AV1 use the standard Moonlight stream; which of them you get depends on the host's encoder and the client's decoder. PyroWave is a separate wavelet codec that needs far more bandwidth, and its client can test the connection before streaming.

PyroWave's recommended rates, from desktop and game test scenes on AMD, are **277 Mbps at 720p60, 399 Mbps at 1080p60 and 1593 Mbps at 4K60**. The stream card warns below those rates, and more strongly below **139, 187 and 747 Mbps**, where loss becomes severe. Leave headroom for packet overhead and recovery data: 4K60 needs more than gigabit Ethernet. Use HEVC or AV1 when the client or network cannot carry the rate ([Capture and video](configuration.md#capture-and-video)).

Moonlight's **YUV 4:4:4** option needs an encoder that produces 4:4:4. AMD's hardware encoders produce 4:2:0 only, so Moonlight streams 4:2:0 and warns that the host does not support YUV 4:4:4. For full-resolution colour, use PyroWave, or Nightjar, which receives 4:4:4 packed into an ordinary HEVC or AV1 picture.

**HDR.** Turn on HDR in the client and make sure the **streamed display** supports HDR in Windows. Under **All settings → Display & HDR**, the defaults **Display HDR: Match the stream** and **HDR request: Follow the device** are the right starting point. Forcing display HDR does not turn an SDR stream into HDR. Turn on the game's own HDR mode where it has one. Devices that ask for them also get Dolby Vision (HEVC and AV1), HDR10+ scene data or HLG on the same stream; other devices get HDR10. The stream card shows the codec and an **HDR** badge; check the picture on the client's own screen as well ([HDR troubleshooting](troubleshooting.md#hdr-looks-washed-out-too-bright-or-different-between-clients)).

## Virtual displays and updates

**Settings → Picture → Virtual display** chooses **One for each device**, **One shared by all devices**, or **Off, stream a monitor**. **All settings → Display & HDR → Virtual display layout** can also make the virtual display primary or isolate it from the other monitors. Settings for one app or one device can override the choice ([Displays and RTSS](configuration.md#displays-and-rtss)).

Updates are offered, never forced. **Help → Updates** has **Check now** and, for the installed service, **Install when idle** and **Install now**; the tray menu has **Check for updates**. **Settings → Updates → Install updates automatically** is off by default. **All settings → General → Include pre-releases** also offers release candidates.

**Install when idle** and automatic updates wait until no stream, pending connection, remote monitor or host-launched app is running, then for one further minute of idle time. A disconnected Desktop session can still hold an app open; close it from the client or the console if an update stays queued. **Install now** installs immediately: the stream ends, games started from a client close while Rubylight restarts, and you reconnect once it is back. Portable users download the new archive from the release page.

## Bring an existing profile

On the first portable launch, **Yes** in the import prompt copies an existing Vibepollo or Apollo profile. Select the folder that contains `sunshine.conf`, or the installation folder that contains `config\sunshine.conf`. **No** starts a new profile.

The import brings settings, paired devices, the host identity, the console sign-in, the app library and covers. The original profile is left in place. The destination must be empty: Rubylight never overwrites a populated profile, and the prompt appears only before the portable profile exists.

| How it runs | Profile folder |
| --- | --- |
| Installed Windows service | `%PROGRAMDATA%\Butterpollo\config` |
| Portable launcher | A `config` folder next to the executable if there is one, otherwise `%LOCALAPPDATA%\ButterpolloRust\config` |

Opening the launcher again returns to the running profile's console; the installed launcher uses the service's profile. `--config-dir` selects another profile ([Multiple instances](configuration.md#multiple-instances)).

## Upgrading

If you play on the host PC itself, install between gaming sessions. **Install when idle** sees Rubylight's streams and the apps it launched, not every game started outside Rubylight. Elevation prompts can take focus, and driver installation can briefly reconnect displays or devices even when nothing is streaming.

- **Streams end.** Installing ends streams and closes host-launched apps. The setup window warns first when Rubylight reports a stream; a quiet install refuses without `--end-streams`. Vibepollo, Apollo and Sunshine cannot report streams, so setup stops them without asking.
- **Back up the profile.** Copy the whole profile folder from the table above, or the previous host's `config` folder: `sunshine.conf`, `apps.json`, covers, both identity files in `credentials`, `sunshine_state.json`, `sunshine_credentials.json` if present, `vibeshine_state.json`, and any files named by custom paths in the configuration. Keep the backup private: it holds credentials and device certificates.
- **Install.** Run the new installer, or use **Help → Updates → Install when idle** or **Install now**. Settings, unknown configuration keys, paired devices, credentials, apps, device and display settings and saved session data stay in the profile; active streams do not survive the restart. Installed drivers stay available. The legacy AMD encoder name `amdvce_experimental` still selects AMF. A saved Xbox 360 or DualShock 4 (ViGEmBus) controller type loads as the VHF Xbox One or DualShock 4 pad; ViGEmBus is no longer used.
- **Portable.** Close the portable host and extract the new archive into a separate folder. The launcher reuses `%LOCALAPPDATA%\ButterpolloRust\config` by default; copy an adjacent `config` folder next to the new executable, or keep passing the same `--config-dir`. Keep the old archive and the backup until the new version works, and do not open an older version against a newer profile.
- **From Butterpollo.** Rubylight was called Butterpollo up to 2.0.0-rc.29. Upgrading renames what Windows shows (the Start menu, Apps, the service's display name and the firewall rule) and keeps everything else: the install folder, the `ApolloService` service, the profile folders and file names such as `butterpollo.exe`. The launcher is **Start Rubylight.exe**, which replaces **Start Butterpollo.exe**.
- **Another host's profile.** If setup cannot tell which profile to import, or Rubylight already has settings next to another host's profile, settle that before removing either host. Imports keep recognised files and unknown fields; old logs and oversized or linked optional files can be skipped, so keep the original backup.
- **Check the result.** The console opens, existing clients connect, apps and covers appear, and display and controller settings still apply. Restart Playnite if its plugin was updated. If an update fails, keep the profile's `updates` folder and `update-result.json`. Installer downgrades are refused.
- **Uninstalling.** A normal uninstall keeps the profile and the drivers. A factory reset (`--factory-reset` in a quiet uninstall) deletes the profile, and `--remove-drivers` removes the virtual display and VHF drivers. usbip-win2 stays installed; remove it in Windows Settings if nothing else uses it.

[Features](features.md) lists everything Rubylight does, and [Performance](performance.md) how it is measured.
