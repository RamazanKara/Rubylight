## desktop
- Linux Wayland: HDR streams describe their colours to the compositor at the display's own reference white and peak, so KDE Plasma and other compositors no longer rescale or tone map each frame, and on compositors that allow it the stream can go straight to the display.
- Linux Wayland: on KDE Plasma 6.5 and later, HDR streams are drawn in the desktop's own HDR format, so the desktop no longer converts each frame and can show the stream straight on the display.
- Linux: new setting Video → HDR brightness follows the desktop. Off by default: HDR streams show at the brightness the PC sends. On: they get brighter or dimmer with the desktop's brightness, as before.
- Linux Wayland: when the compositor asks for a different buffer layout to put the stream straight on the display, Nightjar now gives it one.
- Linux Wayland: with V-Sync off, VRR streams show each picture as soon as it is ready instead of queueing it.
- Linux: after an HDR stream, the launcher is drawn at the desktop's own white instead of too bright or too dim.
- Linux Wayland: the end-of-stream log says what stood for or against showing the stream straight on the display.
