## en
- Fewer audio crackles in the first seconds of a stream on phones and handhelds with a small audio buffer: the buffer now starts large enough for the 10 ms bursts in which PCs send their sound (14 ms instead of 8 ms on a Retroid Pocket 6).
- A Wi-Fi dropout longer than the audio buffer can grow to cover no longer raises the audio delay for minutes. The dropout is bridged as before; the buffer grows only for gaps it can cover.
