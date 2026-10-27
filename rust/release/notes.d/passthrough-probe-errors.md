# Passthrough probe errors
- When the host can't check its PCM and Dolby Digital audio while a stream starts (FFmpeg's folder not found, or the check fails outright), the client now gets an error answer with the reason, and the host log says why, instead of a connection the host drops without a word. With PCM and Dolby Digital audio off the check isn't needed and streams start as before.
