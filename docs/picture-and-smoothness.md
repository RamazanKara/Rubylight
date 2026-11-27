# Picture and smoothness

[Rubylight docs](README.md) › [Picture and smoothness](README.md#picture-and-smoothness) · [Configuration](configuration.md#capture-and-video) · [Performance](performance.md)

The device asks for the stream it wants: resolution, frame rate, bitrate, codec and HDR are set in Nightjar or Moonlight, not on the PC. Rubylight's defaults then capture, encode and send that stream with the least delay it can, so most people change nothing here. This page says what to change when the picture or the motion is not what you expect.

## Start with the defaults

The stream preset **Balanced** (**Settings → Picture → Stream preset**) is the default set. Two other presets change the encoder for every stream, or for one device or game:

- **Lowest latency** keeps every frame, recovery frames included, small, so frames encode and arrive sooner. It costs some picture quality after scene cuts and losses. Use it for fast games on a link that keeps up.
- **Reference quality** lets recovery frames be as large as they need. They take longer to send, so use it only on a fast, steady link.

A setting you change by hand always wins over the preset ([Stream presets](configuration.md#stream-presets)).

## Codec, HDR and sharp text

- **Codec.** H.264 works everywhere; HEVC and AV1 give a better picture at the same bitrate where the device decodes them, and carry HDR. PyroWave keeps full colour and suits a fast wired local network, with Nightjar ([Choose your stream format](getting-started.md#choose-your-stream-format)).
- **HDR.** Turn on HDR in the client and in the game. The PC's streamed display switches to HDR for the stream by itself. Dolby Vision, HDR10+ and HLG go to devices that ask for them ([Codecs, full chroma and HDR](configuration.md#codecs-full-chroma-and-hdr), [HDR troubleshooting](troubleshooting.md#hdr-looks-washed-out-too-bright-or-different-between-clients)).
- **PyroWave at a low bitrate.** Below the rate where its test scenes kept detail, a PyroWave stream still starts at the rate the client asked for and warns on the stream card and in Nightjar that text and textures may lose detail. Raise the bitrate, lower the resolution or frame rate, or use HEVC or AV1.
- **Text colour in HDR.** In HDR10 and HLG streams the host mixes ClearType's coloured glyph edges by their light, so small text keeps its colour instead of turning magenta.
- **Sharp text.** Small coloured text needs full colour (4:4:4). On NVIDIA, Moonlight's **YUV 4:4:4** works where the encoder offers it; on Radeon, use PyroWave or Nightjar's **Sharper colored text**. It adds encode time and pays off only at higher bitrates, so leave it off for games.

## Displays

By default each device gets a **virtual display** of its own, at the device's resolution and refresh rate, on Windows 11 23H2 and later. Games open on it, and the PC's monitors turn off while you stream so games can't open on them; they come back when the stream ends.

- **Virtual display as the main screen.** With **Extend — make virtual display primary**, the virtual display stays the main screen: when Windows changes the displays by itself, the stream puts it back within about a second. A main screen you pick yourself in Windows stays yours.
- **Keep your monitors on.** Choose an **Extend** layout under **All settings → Picture → Streamed display → Virtual display layout**. Games may then open on a monitor instead of the stream.
- **Stream a monitor instead.** Choose **Off, stream a monitor** under **Settings → Picture → Virtual display**. The monitor switches to the device's resolution and the stream's frame rate where it has that mode, and back afterwards; to leave it exactly as it is, set **Monitor setup** to Disabled.
- **One device, its own choice.** Nightjar can ask, for each PC, for the virtual display alone, beside your monitors, or none.
- **Stream a game's window.** **What to stream** (**All settings → Picture → Capture**) can be **The app’s window**, or **The app’s window, the display while it’s in the background** (`window_focus`): the game's window while it is in front, and the whole display while you Alt+Tab or click outside it, without restarting the stream. A game or a device can have its own choice ([The app's window in place of a display](configuration.md#the-apps-window-in-place-of-a-display)).

If the monitors stay off after a stream, **Advanced → Troubleshoot** brings the saved layout back ([Displays and RTSS](configuration.md#displays-and-rtss), [Monitors stay on, or the display layout does not return](troubleshooting.md#monitors-stay-on-or-the-display-layout-does-not-return)).

## Smooth motion

- **Real VRR** (Nightjar). On a device with a VRR screen, Nightjar shows each frame at the moment the game presented it on the PC, so the screen keeps the game's rhythm instead of the network's ([Real VRR](../client/docs/real-vrr.md)).
- **Frame Warp** (Nightjar). A mouse, right-stick or gyro camera turn shows on the device's next screen refresh, and the game's HUD stays in place. 6.5 has Frame Warp 2 only, so Nightjar and Rubylight must both be current. The host only does the work for a device that asks ([Frame Warp](frame-warp.md)).
- **Frame limits.** On a virtual display Rubylight caps games at the stream's frame rate through RTSS when RTSS is installed, so frames are evenly spaced. A game, a device or every stream can have a limit of its own ([Displays and RTSS](configuration.md#displays-and-rtss)).
- **Driver settings.** A game's **Graphics driver** panel applies a Radeon or NVIDIA driver profile while it streams and puts the PC's own values back afterwards ([Radeon settings per game](configuration.md#radeon-settings-per-game)).

## An unsteady network

Rubylight protects the stream by itself:

- When frames are lost beyond what error correction repairs, it lowers the bitrate to what the link carried, and returns to your rate once the link is clean again. A link that carries the stream is never touched ([Lower bitrate when the link cannot carry it](configuration.md#lower-bitrate-when-the-link-cannot-carry-it)).
- Devices that say they are on Wi-Fi or mobile data get their packets paced from the first frame.
- Video and audio are marked so Wi-Fi sends them first. Turn **Prioritize stream traffic** off only for a router that mishandles marked packets ([DSCP](configuration.md#prioritize-stream-traffic-dscp)).
- A device that chooses its own error correction, such as Nightjar, adds recovery data where the link loses packets ([Error correction chosen by the device](configuration.md#error-correction-chosen-by-the-device)).

If the stream still stutters, lower the bitrate on the device, use a cable or 5 GHz Wi-Fi, and read [The stream breaks up, drops or times out on the network](troubleshooting.md#the-stream-breaks-up-drops-or-times-out-on-the-network). The stream card on **Home** shows what the host changed and why.

## NVIDIA extras

On a GeForce RTX card, **RTX HDR** turns an SDR game into an HDR stream and **RTX Video enhancement** cleans up and sharpens the picture, after the console downloads NVIDIA's components. Both are off until you turn them on for the host or an app; each costs GPU time ([RTX HDR and RTX Video enhancement](configuration.md#rtx-hdr-and-rtx-video-enhancement), [NVENC](nvenc.md)).

## Measure your own setup

[Performance](performance.md#measure-your-setup) explains what the stream card and the client's overlay measure, and how to compare settings on your own PC.
