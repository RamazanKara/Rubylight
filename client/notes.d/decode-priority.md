## en
- With "Prioritize stream traffic" on, the classic streaming engine's thread that feeds the video decoder now gets the decoder's priority on every decoder, so a busy phone delays frames less.
- New Debug setting, off by default: "Decoder priority for receiving" does the same for the New streaming engine's receive thread when it feeds the decoder directly.

## desktop
- On Windows the stream's receive, decode, audio and control threads now get a raised priority, so other busy programs delay frames less.
- On Linux, including the Flatpak on Bazzite and SteamOS, the stream and controller threads are now raised through the desktop portal or RealtimeKit when the system refuses it directly, instead of staying at normal priority.
