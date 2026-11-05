# Browser streaming and guests

- The console's Stream page now works in Firefox on the PC itself. With the console opened at `127.0.0.1` or `localhost`, the host offered only that address, and Firefox doesn't send anything to a loopback address, so every stream failed after 5 seconds with "The connection to the host failed."
- A browser on the PC itself keeps its monitors on: the `exclusive` display layout runs as `extended_primary` for such a stream, since the browser showing it is on one of them.
- When a browser stream or a guest link can't connect, the page now says what it saw within 8 seconds, in plain words: the browser never tried, the PC didn't answer at this address and UDP port, or the encrypted connection failed, each with its fix. The host's log records the same.
