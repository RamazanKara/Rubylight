## desktop
- Linux: Nightjar runs without a desktop session. On a text console it draws straight to the display through DRM/KMS with input from evdev, and the whole app and command-line starts work there. Settings › Advanced › Display output chooses it, and `--kms` or `--no-kms` override it for one start.
- Drawing straight to the display, a stream's pictures go onto a display plane without a copy, with HDR10, variable refresh rate, content type Game and tearing flips with V-Sync off; the overlay and stream menu go on a plane above (Video planes).
