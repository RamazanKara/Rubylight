# Steam Deck and Linux handhelds

[Documentation](index.md) · [Everything Nightjar does](features.md) · [Android and desktop](CLIENT-PARITY.md)

Nightjar for Linux runs on the Steam Deck and other Linux handhelds as a Flatpak. It starts from Steam in Game Mode, works with the built-in controller alone, and shows HDR where gamescope offers it.

## Install

Every Nightjar release carries `nightjar-desktop-X-x86_64.flatpak` (X is the version) on the [releases page](https://github.com/RamazanKara/nightjar/releases), listed in its `SHA256SUMS` file.

1. Switch to Desktop Mode (Steam button → Power → Switch to Desktop).
2. Download the `.flatpak` file, open Konsole in the download folder and run:

   ```sh
   flatpak install --user nightjar-desktop-X-x86_64.flatpak
   ```

   Flatpak fetches the freedesktop runtime from Flathub the first time.
3. For HDR in Game Mode, also install gamescope's Vulkan layer:

   ```sh
   flatpak install --user flathub org.freedesktop.Platform.VulkanLayer.gamescope//24.08
   ```

To update, install the next release's `.flatpak` the same way; settings and paired PCs stay. They live in `~/.var/app/io.github.ramazankara.nightjar/config/nightjar`.

Nightjar is not on Flathub: Flathub builds every app whose source is public from that source on its own servers, and this bundle repackages the release archive instead.

## Add a Non-Steam Game

1. In Desktop Mode, open Steam and choose **Games → Add a Non-Steam Game to My Library**.
2. Tick **Nightjar** in the list and choose **Add Selected Programs**.
3. Switch back to Game Mode (the **Return to Gaming Mode** icon on the desktop). Nightjar is in the library under **Non-Steam**.

Optionally give it artwork in the game's **Properties**. A PC or game can also start directly: put `--host <PC> --app <game>` after `io.github.ramazankara.nightjar` in the **Launch Options**.

## Game Mode controls

Nightjar opens fullscreen without a window border. The controller drives everything:

| Where | Button | Does |
| --- | --- | --- |
| Launcher | D-pad or left stick | Moves the focus; the first press focuses the page's first item |
| Launcher | A / B | Selects / goes back or closes |
| Launcher | L1 / R1 | Switches tabs |
| Stream | **Start+Back** | Opens the stream menu |
| Stream menu | B | Returns to the stream |

Steam keeps the Steam button in Game Mode, so the Hold Guide shortcut does not reach Nightjar there; Start+Back opens the stream menu instead, and the first seconds of each stream say so. The trackpads move the pointer, and the pointer hides again as soon as the controller is used. The Quick Access menu (…) still opens Steam's own overlay.

## HDR

With HDR turned on in the Steam Deck OLED's display settings (or gamescope started with `--hdr-enabled` on another handheld), Nightjar presents HDR through gamescope: scRGB where gamescope offers it, HDR10 (PQ) where it offers only that. The log names the renderer and the choice (`nightjar: renderer "gpu": swapchain offers ...`). Without HDR output, an HDR stream plays in SDR and its first seconds say so.

## What the Flatpak can't do

- **Forwarding webcams and audio devices.** Most USB devices are forwarded from the Flatpak as from the other packages. Webcams and audio devices go through Linux's USB/IP tools instead (usbip-host, bound through `pkexec` and Nightjar's helper as root), which the sandbox can't reach; the **USB devices** list says so for them. Use the `.deb`, `.rpm` or AppImage with your distribution's usbip package for those.

The Flatpak has the network, the GPU, controllers and audio, so discovery, Wake-on-LAN, controllers, the microphone and hardware decoding work as in the other Linux packages. The Steam Deck's AMD graphics decode with the runtime's own drivers; on Intel graphics, VA-API in a Flatpak needs `flatpak install --user flathub org.freedesktop.Platform.VAAPI.Intel//24.08`.
