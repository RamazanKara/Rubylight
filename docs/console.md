# Home and the session menu

Home, a game's page and the in-stream **Session and graphics** menu use Rubylight's native API. They appear only for a [Rubylight](https://github.com/RamazanKara/Rubylight) 3.0 host, alpha.1 or newer. The Radeon panel needs the next Rubylight 3.0 alpha and stays hidden on alpha.1. Sunshine, Apollo, Vibepollo, older Rubylight and other Moonlight-compatible hosts do not offer the API, so they keep the classic PC and game grid, and nothing in the classic screens changes.

## Home

Nightjar opens on Home when a paired PC supports the API. Home has three parts:

- **Continue playing**: games that are running or suspended, with a Running or Suspended badge.
- **Library**: the games of every paired PC with covers. A game on several PCs is one card ("On 2 hosts").
- **Hosts**: your PCs, with their game counts. An offline PC keeps the games it last reported and says so.

Turn Home off with **Open on Home** in settings to start on the list of PCs. **Hosts** in the Home menu opens that list at any time.

## PCs on the Windows and Linux client

The header lists **Home**, **PCs** and **Settings** in that order, with the current one underlined. **Home** shows only when some PC has a library. The destinations replace each other: Back from PCs or Settings goes to Home (or stays on PCs when Home is off), however many you visited. Profiles, Controller mappings and Keyboard shortcuts open from Settings and Back returns to the row that opened them. Settings lists **Video**, **Display & image**, **Controls**, **Audio**, **Streaming**, **App** and **Advanced**, with a chip for each at the top that jumps to it.

A PC's options are one list, opened with **F2** (the controller's West button), **Shift+F10**, a right-click or the three dots on its card. The entries depend on the PC: **Open library** or **Pair**, **Pair with one-time PIN** (hosts that offer it), **Quit running app**, **Wake up** (offline with a known MAC), **PC settings**, **Rename**, **Unpair**, **Check again** and **Remove PC**. **Ctrl+N** adds a PC by address.

- **Rename** keeps a name on this device only. The PC's own name keeps updating underneath, and clearing the field (or typing the PC's own name) brings it back. The name is at most 40 characters.
- **Unpair** and **Remove PC** ask first. Removing deletes the PC, its settings and its saved library from this device; the PC still lists this device until you unpair it.
- A pairing that fails says why in a dialog with **Try again**: the PC is offline, a game is running on it, the PIN or one-time PIN was wrong, another request is pending, or the network blocked it. A failed unpair or wake says so the same way.

## A game's page

Pick a card to see the PC the game runs on and the stream settings it will use (resolution, frame rate, bitrate, and which profile layer set them), then **Play** or **Resume**. If another game is running on that PC, the page says that playing this game closes it. **Close game** appears when the PC has granted this device Host control. Without it you get the classic Quit.

## Session and graphics

Press Back while streaming to open the stream menu, then **Session and graphics**:

- **Session**: Resume, Disconnect, End session, and Close game with Host control (otherwise the classic Quit game).
- **Connection**: the host, codec, resolution and frame rate, bitrate, and your link (Wi-Fi generation, band and speed, Ethernet or mobile data, with the round trip).
- **Radeon graphics**: the AMD driver features the host exposes, such as Anti-Lag and Radeon Chill.

The page reads the host only while it is open, and the Radeon state at most once a second, never on the video, audio or input threads. Back closes it.

### Radeon panel

Each setting shows what the host says about it:

| State | What you see |
| --- | --- |
| Configurable | A switch, slider or choice you can change |
| Read only | The value, not editable |
| Needs setup in AMD Software | The value and what to do once on the PC |
| Not supported | A note that the GPU or driver does not support it |
| Temporarily unavailable | The reason, and Nightjar asks again |

You can change a setting only when the host marks it configurable, your device has Host control and you own the running session. Turn on Host control for the device in the Rubylight console. After a change, the panel shows what the host reads back. A change made in a stream is undone when the session ends (End session, Close game, grace expiry, unpair or host shutdown), and the panel says so next to the setting. Disconnect keeps the change until you resume or end the session. A setting Nightjar does not know yet is still listed, drawn from its type. If the driver refuses a change, the reason appears next to that setting.
