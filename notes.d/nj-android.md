## en
- Android: opening the stream settings no longer freezes on devices whose media service is slow to list its decoders. The decoder check runs in the background and the list is read once per app start.
- Android: a decoder crash now writes the decoder's name, the video format and the frame counts to the log.
- Android: a display that reports no HDR peak brightness now gets one estimated from Android's HDR headroom (Android 14 and later), so the host tone-maps for the panel instead of a default. Sunshine-family hosts now get the display's luminance at launch too.
- Android: a PC added by name no longer costs a DNS lookup to tell whether it is on the local network.
