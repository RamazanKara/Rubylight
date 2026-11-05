# Browser streaming and guests

- The console's Stream page now works in Firefox on the PC itself. With the console opened at `127.0.0.1` or `localhost`, the host offered only that address, and Firefox doesn't send anything to a loopback address, so every stream failed after 5 seconds with "The connection to the host failed."
- A browser on the PC itself keeps its monitors on: the `exclusive` display layout, and `ensure_only_display` on a physical display, run as `extended_primary` for such a stream, since the browser showing it is on one of them.
- When a browser stream or a guest link can't connect, the page now says what it saw, in plain words: the browser never tried or the PC didn't answer at this address and UDP port (within 8 seconds), or the encrypted connection failed (within 30 seconds), each with its fix. The host's log records the same.
