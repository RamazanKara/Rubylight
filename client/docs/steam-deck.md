# Steam Deck and Linux handhelds

[Nightjar docs](index.md) › [Controllers](index.md#controllers) · [Everything Nightjar does](features.md) · [Android and desktop](CLIENT-PARITY.md)

Nightjar for Linux runs on the Steam Deck and other Linux handhelds as a Flatpak. Added to Steam, it runs under gamescope in Game Mode, works with a controller alone, and shows HDR where gamescope offers it.

## Install

Every Nightjar release carries `nightjar-desktop-X-x86_64.flatpak` (X is the version), and `nightjar-desktop-X-aarch64.flatpak` for ARM handhelds, on the [releases page](https://github.com/RamazanKara/nightjar/releases), listed in its `SHA256SUMS` file.

1. Switch to Desktop Mode (Steam button → Power → Switch to Desktop).
2. Download the `.flatpak` file, open Konsole in the download folder and run:

   ```sh
   flatpak install --user nightjar-desktop-X-x86_64.flatpak
   ```

   Flatpak fetches the freedesktop runtime from Flathub the first time.
3. For HDR in Game Mode, also install gamescope's Vulkan layer:

   ```sh
   flatpak install --user flathub org.freedesktop.Platform.VulkanLayer.gamescope//25.08
   ```

To update, install the next release's `.flatpak` over the old one with `--reinstall` (without it Flatpak stops at "already installed"):

```sh
flatpak install --user --reinstall nightjar-desktop-X-x86_64.flatpak
```

Settings and paired PCs stay. They live in `~/.var/app/io.github.ramazankara.nightjar/config/nightjar`.

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
| Stream | **Start+Back** | Opens the Control Center |
| Control Center | LT / RT | Steps through its modules |
| Control Center | B | Returns to the stream |

In Game Mode, when gamescope's frame queue holds a V-Sync stream back after a game changes its frame rate, Nightjar notices within a few seconds and shows the newest frame at each refresh instead, still without tearing, and says so. This applies to Lowest latency, Balanced and PC's frame times; a gamescope frame limit keeps its own pacing.

Steam keeps the Steam button in Game Mode, so the Hold Guide shortcut does not reach Nightjar there; Start+Back opens the Control Center instead, and the first seconds of each stream say so. The trackpads move the pointer, and the pointer hides again as soon as the controller is used. The Quick Access menu (…) still opens Steam's own overlay.

Gyroscope and accelerometer motion controls work automatically in Game Mode when **Motion controls** is enabled in Settings, even while Steam exposes the Deck through Steam Virtual Gamepad. To use the physical Deck's back grips and trackpads directly through SDL instead (with Steam Input disabled for Nightjar), put `SDL_GAMECONTROLLER_ALLOW_STEAM_VIRTUAL_GAMEPAD=0 SDL_GAMECONTROLLER_IGNORE_DEVICES=""` in Steam's **Launch Options** before `%command%`.

## HDR

With HDR turned on in the Steam Deck OLED's display settings (or gamescope started with `--hdr-enabled` on another handheld), Nightjar presents HDR through gamescope: an HDR stream as HDR10 (PQ) where gamescope offers it, else scRGB. The log names the renderer and the choice (`nightjar: renderer "gpu": swapchain offers ...`). Without HDR output, an HDR stream plays in SDR and its first seconds say so.

## What the Flatpak can't do

- **Kernel `usbip-host` binding for USB devices.** Most USB devices — including webcams, audio devices and composite controllers with isochronous interfaces — are forwarded from the Flatpak through Nightjar's userspace `libusb` path. Binding devices through Linux's kernel `usbip-host` tools (`pkexec` and Nightjar's root helper) is unavailable inside the sandbox; use the `.deb`, `.rpm` or AppImage with your distribution's `usbip` package if you prefer kernel `usbip-host`.

The Flatpak has the network, the GPU, controllers and audio, so discovery, Wake-on-LAN, controllers, the microphone and hardware decoding work as in the other Linux packages. The Steam Deck's AMD graphics decode with the runtime's own drivers; on Intel graphics, VA-API in a Flatpak needs `flatpak install --user flathub org.freedesktop.Platform.VAAPI.Intel//25.08`.

## Not yet run on a Steam Deck

Nightjar's Game Mode runs were made under gamescope on an Intel mini PC with a virtual controller, not on a Steam Deck. Still to check on a real Deck:

- Touching the screen. Nightjar ignores the pointer that SDL makes from touch, but gamescope may pass the Deck's touchscreen on as an ordinary mouse, which Nightjar would then follow.
- The trackpads and HDR on the Steam Deck OLED's own display.
