# Home and the session menu

Home, a game's page and the in-stream **Session and graphics** menu use Rubylight's native API. They appear only for a [Rubylight](https://github.com/RamazanKara/Rubylight) 3.0 host, alpha.1 or newer. The Radeon panel needs the next Rubylight 3.0 alpha and stays hidden on alpha.1. Sunshine, Apollo, Vibepollo, older Rubylight and other Moonlight-compatible hosts do not offer the API, so they keep the classic PC and game grid, and nothing in the classic screens changes.

## Home

Nightjar opens on Home when a paired PC supports the API. Home has four parts:

- **Favourites**: games you starred, first on the page. Long-press a game card (with a D-pad or controller, hold the select button) and choose **Add to favourites**, or use the star on the game's page. A favourite belongs to one PC, so the same title on two PCs is two entries. Favourites are kept when the app updates, and a favourite whose PC is offline stays on Home with its saved cover.
- **Continue playing**: games that are running or suspended, with a Running or Suspended badge.
- **Library**: the games of every paired PC with covers. A game on several PCs is one card ("On 2 hosts").
- **PCs**: your PCs, with their game counts. An offline PC keeps the games it last reported and says so. Each card has three dots at its end with the PC's options, the same list as on the PCs screen (see below); with a D-pad, press Right on the card to mark the dots, then Center.

Turn Home off with **Open on Home** in settings to start on the list of PCs. The shell bar under the top bar (Home, PCs, Settings; see below) opens that list at any time. A PC paired from the PCs screen that offers the API lands on Home, where its games are; any other PC opens its game list as before.

## PCs on the Windows and Linux client

The header lists **Home**, **PCs** and **Settings** in that order, with the current one underlined; Tab walks them in that order, then **Add PC** on the PCs screen. **Home** shows only when some PC has a library. The destinations replace each other: Back from PCs or Settings goes to Home (or stays on PCs when Home is off), however many you visited. Start (the controller's menu button) from a library or a game page opens Settings over it, and Back returns to that page. Profiles, Controller mappings and Keyboard shortcuts open from Settings and Back returns to the row that opened them. Settings lists **Video**, **Display & image**, **Controls**, **Audio**, **Streaming**, **App** and **Advanced**, with a chip for each at the top that jumps to it. On a TV and on screens at least 840 dp wide the sections sit in a left pane beside the chosen section, with **Search settings** as its first row; Profiles, Controller mappings, Keyboard shortcuts, GPU driver and the latency benchmark start with focus on their content and share one focus outline. See [Settings](settings.md).

A PC's options are one list, opened with **F2** (the controller's West button), **Shift+F10**, a right-click or the three dots on its card. The entries depend on the PC: **Open library** or **Pair**, **Resume** and **Quit running app** (a game is running and the library is known), **Pair with one-time PIN** (hosts that offer it), **Wake up** (not online, with a known MAC), **PC settings**, **Rename**, **Unpair** (paired and online) or **Pairing help** (not paired), **Check again** and **Remove PC**. **Ctrl+N** adds a PC by address.

- **Rename** keeps a name on this device only, and the stream screens show it too. The PC's own name keeps updating underneath, and clearing the field (or typing the PC's own name or address) brings it back. The field starts empty for a PC that never reported a name. The name is at most 40 characters.
- **Unpair** and **Remove PC** ask first. Unpair asks the PC to forget this device, so it needs the PC online; this device keeps its pairing until the PC has answered. Removing deletes the PC, its settings, its saved library, its covers and its recent games from this device; the PC still lists this device until you unpair it.
- A pairing that fails says why in a dialog with **Try again**: the PC is offline, a game is running on it, the PIN or one-time PIN was wrong, another request is pending, or the network blocked it. A failed unpair or wake says so the same way.

## Language, zoom, contrast and motion on the Windows and Linux client

Four settings under **Settings > App** change how Nightjar looks. They are global: no profile, PC or game overrides them.

- **Language**: **System default**, **English** or **Deutsch**. System default follows the operating system's first language that Nightjar has, and falls back to English. The launcher, dialogs and the stream menu are translated; German is the first language, and the other languages Nightjar Android ships are not in the desktop client yet. The Radeon panel names and the stream's toasts are translated too, except the brand names (Radeon Chill, AMD Fluid Motion Frames, ...) and the error text that comes from the host or the network. A few lines stay in English everywhere: the names of settings an older client does not know (made from the host's ids), the upscaler names, the performance overlay, startup error boxes and log lines.
- **UI zoom**: 85, 100, 115, 130 or 150 percent, for the launcher and the in-stream menu and overlay. **Ctrl+=** and **Ctrl+-** step it, **Ctrl+0** goes back to 100. At 150 percent a 1280x720 window still shows every button; pages scroll instead of clipping.
- **High contrast**: black surfaces, white text, brighter amber, outlines on every control and a thicker focus ring. Body text is at least 7:1 against its background and the focus ring at least 3:1. The normal theme meets WCAG AA (4.5:1) for text.
- **Reduce motion**: **Follow system**, **Reduce** or **Full motion**. Reduce stops fades, slides, scroll animation and the spinning wheel. Follow system reads Windows' "Show animations in Windows" switch. Linux has no single setting Nightjar can read, so there Follow system leaves motion on; pick **Reduce** to turn it off.

## The shell bar

Home, PCs and Settings sit in one bar under the top app bar of each of those three screens. The current one is highlighted. Home is hidden until a PC has a library to show. On a TV the bar and the screens keep clear of the overscan edge.

- The first screen is Home when **Open on Home** is on and a library exists, otherwise PCs. Back from any other of the three goes to that first screen. Back there leaves the app.
- Switching does not stack screens: the first screen is brought back to the front, and any other screen closes when it opens a sibling.
- Help left the PCs toolbar. Find it under Settings, App, Help, press F1, or use the pairing guide.
- Settings applies the Back rule only when the bar opened it; opened with Ctrl+comma or from a menu, Back just closes it. If Home turns on or off while you are in Settings, Back restarts the task on the new first screen.
- Home appears in the bar as soon as the first library is read, without a resume.
- Home's old toolbar menu is gone; PCs in the bar replaces it. Home shows the PC the way the PCs screen does, with the name given to it by Rename.
- A game's page has the bar too, with Home marked as the current destination because the page belongs to Home. Home in the bar closes the page (Home is underneath); PCs and Settings open as they do from Home.

## PCs on Android

Every PC card has three dots at its end. They open the PC's options, as does a long press, a mouse's right button, Menu or Shift+F10, and a controller's X. In a PC's library, X opens the selected game's options the same way. With a D-pad, press Right on a card with nothing further to its right to mark the three dots, then Center. When a key or controller was the last input, the PCs screen says "X or ⋮ for options".

The options are one list: **Open library** or **Pair**, **Resume** and **Quit** (a game is running), **Pair with one-time PIN** (hosts that offer it), **Wake PC** (offline, with a known MAC address), **PC settings**, **Rename**, **Unpair** (paired and online) or **Pairing help** (not paired), and **Remove PC**. An offline PC without a MAC address says "Pair once while the PC is on to enable Wake".

- **Rename** keeps a name on this device only (`HostNames`, keyed by the PC's id), and the library, the stream's title, shortcuts and the TV channel show it. A game's own settings page, a PC's settings page and the Home row say it too. The PC's own name keeps updating underneath. An empty field, or the PC's own name, brings the PC's name back. The name is at most 40 characters. Removing the PC forgets it.
- **Unpair** asks first, then asks the PC to forget this device and clears the library saved from it. The PC stays in the list as not paired.
- **Remove PC** asks first, then deletes the PC, its settings, saved library, covers, hidden games and name from this device. The PC still lists this device until you unpair it.
- A failed pairing, unpair or wake says why in a dialog. **Try again** is offered where a retry can help (a wrong PIN, the network, a failed unpair) and left out where it cannot (the PC is offline, or no MAC address is known). A pairing you cancelled never shows a result afterwards.
- A PC's settings page lists **Rename**, **Unpair** (paired and online) and **Remove PC** under Host, the same actions as the list's options. Remove closes the page.

## Accessibility on Android

Three settings under **Settings > App > Accessibility** change how every screen of the app looks. They are global: no profile, PC or game overrides them, and the stream itself is never scaled or recoloured. A change recreates the screen you are on.

- **Interface size**: 85, 100, 115, 130 or 150 percent. It scales the density of the app's own screens, so a 393 dp phone at 150 percent lays out like a 262 dp one, and the layout qualifiers follow. Touch targets stay 48 dp. The system font size still applies on top. `UiScaleTest` checks PCs and Settings at 150 percent for overflow and for targets under 48 dp.
- **High contrast**: black surfaces, secondary text and outlines in the body text colour, a 2 dp outline on cards, and a 4 dp moon-amber focus ring on every focusable. `ContrastTest` computes the WCAG ratios from the palette and the theme: body and secondary text are at least 7:1 and focus rings at least 3:1 on every surface.
- **Reduce motion**: **Follow system**, **Reduce** or **Full**. Follow system reduces when Android's animation scale is 0. Reduce replaces the TV focus zoom with a still ring and skips the sliding sheet on a TV. Full always animates. A focus ring always shows.

On a TV every stop of a D-pad walk shows a ring, never only Material's faint state layer: buttons, icon buttons, rows and cards all draw one (`FocusVisibilityTest` requires focus to change pixels by at least 3:1 around the edge). A filled amber button gets a ring in the body text colour, because amber on amber would not show. Checked by D-pad on an Android 14 TV emulator, normal and high contrast: PCs opens with **Add PC** focused and Settings with its list focused, never the toolbar, and the screens keep 48 dp by 27 dp clear of the overscan edge.

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
