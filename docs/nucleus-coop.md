# Nucleus Co-op: background streaming and co-op

Rubylight can run a game through [Nucleus Co-op](https://github.com/SplitScreen-Me/splitscreenme-nucleus),
the split-screen launcher. Nucleus starts its own instance of the game for
each player, and Rubylight streams each instance's window to its own device.
That gives two things:

- **Background streaming.** A device plays its own instance of a game while
  someone keeps using the PC. The PC's displays, focus, keyboard and mouse
  stay with the PC user.
- **Co-op.** Several devices play the same game together, each in its own
  instance of it, with up to 4 players per session.

## Background streams

Nucleus Co-op is the only background mode: there is nothing to choose. A
device asks for a background stream as before: **Devices → (device) → Play in
the background** (`background_stream`) or **Library → (app) → Behavior →
Stream in the background** (`background-stream`). Either one runs the app as a
Nucleus Co-op session of its own with one player, and the host log says once
that background streams run through Nucleus Co-op. Only the device that
started it can return to it; nobody else can join a background stream.

A background launch that Nucleus Co-op cannot run is refused with the reason,
for example "Store Game can't stream in the background: Nucleus Co-op starts
games by their .exe, and Store Game does not name one". It never falls back to
streaming the PC's monitors.

### Games without a handler

A background stream needs no handler from the hub. Unless you added the game
at that exact path with a hub handler, the host writes its own one-player
handler into Nucleus Co-op's `handlers`
folder (`Rubylight-<game>-<hash>.js`) and adds the game to your Nucleus Co-op
game list, as Nucleus' own "add game" would. It does that only while Nucleus
Co-op isn't running, and never over a file it did not write. A hub handler for
the game you added at the same path always wins, and co-op still needs one:
once you add the game with a hub handler, the next session removes the host's
handler and its entry from your game list. Another game with an .exe of the
same name (Portal's and Half-Life 2's `hl2.exe`) keeps its own.

The game runs from its own folder: nothing is copied, linked or written there,
and Nucleus Co-op needs no extra tools for it. Its window keeps running and
reading the pad without focus, and it never sees the PC's keyboard or moves
the PC's cursor.

- **The app names the game's .exe, or is a Steam link.** For
  `steam://rungameid/<id>` the host starts the .exe Steam would, with that
  launch option's arguments (Portal is `hl2.exe -game portal`), found from
  the game's install folder and Steam's launch option for Windows, and starts
  Nucleus Co-op with the game's Steam app id (`SteamAppId`, `SteamGameId`), so
  the game runs without restarting through Steam. Steam must know the game:
  one that is not installed, or that Steam's app cache does not list yet, is
  refused with the reason (start it once in Steam). Other launchers' links
  are refused: set the app's command to the game's .exe. The game gets the
  arguments the app's command passes it.
- **The game, not its launcher.** A launcher (`steam.exe -applaunch`, the Epic
  Games Launcher, Playnite, `Fallout4Launcher.exe`, `PlayGTAV.exe` and the
  like) is refused. The small .exe at an Unreal game's root starts the one
  `<Project>\Binaries\Win64\*-Win64-Shipping.exe` instead.
- **On this PC's own drives, and no part of Windows.** A game on a network
  share, or a program in the Windows folder, is refused before the host opens
  it. Your user folders are fine.
- **No anti-cheat.** A game that ships anti-cheat, such as Easy Anti-Cheat,
  BattlEye or Riot Vanguard, or that Steam lists with Valve Anti-Cheat, is
  refused before anything is written: Nucleus Co-op's hooks would look like
  cheating. Well-known ones without anti-cheat files in their folder
  (Counter-Strike 2, Dota 2, Overwatch and others) are known by name. The
  host looks only in the game's own folder, never in a folder many games
  share.
- **The Play together panel says why** before you launch: for an app that
  names its .exe it checks the path, launchers, and anti-cheat by name and in
  the game's folder. A Steam link's game (and Steam's Valve Anti-Cheat flag)
  is checked when it launches, and the launch says why.
- **Controllers only**, as for every background stream.
- **Some games pause anyway** when their window is not in front, despite the
  focus hooks. Those need a hub handler made for them.

A `background_mode` line that an earlier build wrote (`auto`,
`child_session` or `virtual_display`) becomes `nucleus`, and the host log
says so once. A value that is no mode at all refuses background launches and
says why.

Background streaming in a Windows child session was removed: Windows signs a
child session in only with a Windows password, and a Microsoft account that
uses only Windows Hello has none. On a PC where a development build set it
up, the host puts the Windows settings back at its start and deletes the
password that build saved; if that fails, **Maintenance** says why and offers
**Restore again**.

## Installing Nucleus Co-op

Set it up from a game's page in the console: **Games** › the game ›
**Play together** › **Set up co-op**. That, or else the first Nucleus launch,
downloads Nucleus Co-op 2.4.2 (`NucleusApp.zip`, 97 MB, from its GitHub
release) and the panel shows the progress. It
checks the archive's size and SHA-256 and refuses one that doesn't match, then
unpacks it into `C:\ProgramData\Butterpollo\nucleus\2.4.2`, which only SYSTEM
and Administrators can change. Nucleus Co-op's handler hub view is a WebView2
browser, which runs without administrator rights, so its data lives in the
signed-in user's `AppData\Local\Butterpollo\nucleus-webview` instead. To use your own copy instead, set
`nucleus_path` to the folder that holds `NucleusCoop.exe`. Nucleus Co-op runs
elevated, so that folder, the exe and the folder's parent must be ones only
SYSTEM and Administrators can change (under `C:\Program Files`, for example);
a copy in a folder you can change without elevation is refused. The game page
and Help show the install state and any download or check error. While
Nucleus Co-op cannot be installed, launches answer 503 with the reason.

Nucleus Co-op launches a game through its **handler** for that game. Handlers
come from Nucleus Co-op's handler hub and are your own content: Rubylight
ships none. When a game has none, its Play together panel says so: **Open
Nucleus Co-op on this PC** opens Nucleus Co-op on the PC's screen, where you
search its handler hub for the game, download the handler and add the game.
The panel moves on by itself once the handler is there. **Close Nucleus
Co-op** beside it closes that window again, and the next co-op or background
stream closes it by itself first; if it won't close (it is asking you
something), the stream is refused until you answer it at the PC. Only
Nucleus Co-op itself is closed: what it started (a browser, a game) is left
alone. A Nucleus
Co-op you started yourself is never closed: Open and launches are refused
while it runs. Co-op in a game without a handler is refused
with "No Nucleus Co-op handler for <exe>: add one in Nucleus Co-op's handler
hub"; a background stream of it uses Rubylight's own handler
([Games without a handler](#games-without-a-handler)).

Rubylight writes a Nucleus profile named **Rubylight** for each session and
loads it. When a start times out with "open Nucleus Co-op and load the profile
Rubylight once", do that on the PC and launch again.

The host must run as the Windows service (the installer's default), because
Nucleus Co-op runs elevated as the signed-in user. A portable host refuses
these launches and says why. The Windows account signed in on the PC must be
an administrator that Windows gives an elevated token: for a standard account,
or with Windows' Administrator protection on (which elevates only into a
separate account after a Windows Hello prompt), launches and **Open Nucleus
Co-op on this PC** are refused and say so.

## Playing co-op

Each app whose game has a handler gets a second library entry, **<app>
(Co-op)**, which sorts next to the game. The game's Play together panel says
how many players it seats and can hide the entry (**Show "<app> (Co-op)" in
the library**, the app setting `coop-entry`). The first device that launches it starts a session with
one slot per player: the handler's maximum, capped at 4 and at
`nucleus_coop_players` (1 to 4, 4 by default). Every other device that
launches the same entry joins the running session in a free slot. Any
Moonlight client can join this way.

Joining a session that someone else started needs **Can join co-op games**
(`coop_join`) on the device's page in the console. A device that never set it
follows Launch apps, so every device with Play games joins. Joining launches the
co-op entry, so a device without Launch apps (Watch only) never joins, and the
console keeps the switch off for it and leaves it out of the invite list. An explicit off
refuses the join with "This device may not join co-op games: turn on “Can join
co-op games” for it in the Rubylight console". A device that holds a slot can go
back to it while it may join; turning it off ends the device's stream and keeps
it out. The device that started the session needs no permission to return. When
a device's stream ends, its slot is held for 120 s so it can come back. A
player's Quit ends only its own stream: the other players keep playing.
Sessions list who plays only to devices that may join.

Home in the console shows each running co-op game as a card: its players, an
away device's remaining 120 s, **Remove** (asks first) and **End co-op** (asks
first). While a slot is free, the card lists the paired devices with **Let
<device> join** for one that may not yet; a removed device is not offered
again. A background stream gets a card too, with **End stream** only.

Nightjar lists the running co-op sessions on a PC with their free slots and
this device's own seat: on Android under **Join co-op** in the PC's menu, on
the desktop on the PC's details page. **Join** (or returning to your own seat)
takes the slot and starts the stream. A refusal is shown with the PC's own
words.

Ending a session stops Nucleus Co-op and every instance, and ends each
player's stream. A session also ends when every player has left, when an
instance crashes, or when its start fails. For a minute after, Home says
which ended last and why, and the log keeps it. Install troubleshooting is in
the Nucleus Co-op panel under **Help**.

## Limits

- **Controllers only.** Each slot gets one virtual Xbox controller, so up to 4
  XInput players. The device's first controller drives its slot. Keyboard,
  mouse, touch and pen are not sent to the game, and the stream says so.
- **Window capture.** Each instance's window is captured with Windows Graphics
  Capture (Windows 10 1903, build 18362, or later), and the window is parked
  away from the PC's monitors. Your displays and their layout are not touched.
- **Sound.** Each stream carries its own instance's sound. Separate sound per
  player needs Windows build 20348 or later. On older builds the host says so.
- One Nucleus Co-op session at a time: a second one is refused while Nucleus
  Co-op is busy with another game.
- **Compatibility settings come back.** As it starts each instance, Nucleus
  Co-op sets the game's .exe to its DPI mode in your Windows compatibility
  settings (Properties › Compatibility), replacing what you set there. The
  host keeps those settings from before the session and puts them back
  exactly when it ends, also after a crash or a failed start. Should the
  host stop mid-session, the next session puts them back before it starts.

## Status

Covered by unit tests with fakes: install and hash checks, handlers and
profiles, the co-op session (slots, join, leave, the held slot, kick, crash,
end), controller routing, and Nightjar's list and join. The hardware runs (the
install, a background stream while the PC is in use, and a two-client co-op
session) are part of the release pass; the [hardware matrix](hardware-matrix.md)
shows their results.
