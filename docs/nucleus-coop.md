# Nucleus Co-op: background streaming and co-op (Experimental)

Rubylight can run a game through [Nucleus Co-op](https://github.com/SplitScreen-Me/splitscreenme-nucleus),
the split-screen launcher. Nucleus starts its own instance of the game for
each player, and Rubylight streams each instance's window to its own device.
That gives two things:

- **Background streaming.** A device plays its own instance of a game while
  someone keeps using the PC. The PC's displays, focus, keyboard and mouse
  stay with the PC user.
- **Co-op.** Several devices play the same game together, each in its own
  instance of it, with up to 4 players per session.

Both are **Experimental**. The other background mode is a
[Windows child session](background-child-session.md).

## Which background mode runs

**Settings › Displays › Background mode** (`background_mode`) picks how a
background stream runs:

| Setting | Mode |
| --- | --- |
| Unset (the default), **Nucleus Co-op** (`nucleus`) | Nucleus Co-op |
| **Automatic** (`auto`) | A child session where this PC can run one, otherwise Nucleus Co-op |
| **Child session** (`child_session`) | A child session |

The older value `virtual_display` reads as `nucleus`, and the host log says so.
The console's Maintenance page and the host log show the mode that runs and
why, for example "Background streaming (Nucleus Co-op)" or "Background
streaming (child session)". A mode that this PC cannot run is not swapped for
another one. The launch is refused with the reason, for example "Background
streaming is set to a child session, which this PC cannot run (why): choose
Automatic or Nucleus Co-op in the settings".

A device still asks for a background stream as before: **Devices → (device) →
Play in the background** (`background_stream`) or **Library → (app) →
Behavior → Stream in the background** (`background-stream`). Either one now runs
in the chosen mode.

## Installing Nucleus Co-op

You don't install anything up front. At the first Nucleus launch the host
downloads Nucleus Co-op 2.4.2 (`NucleusApp.zip` from its GitHub release). It
checks the archive's size and SHA-256 and refuses one that doesn't match, then
unpacks it into `C:\ProgramData\Butterpollo\nucleus\2.4.2`, which only SYSTEM
and Administrators can change. To use your own copy instead, set
`nucleus_path` to the folder that holds `NucleusCoop.exe`. Maintenance shows
the install state and any download or check error. While Nucleus Co-op cannot
be installed, launches answer 503 with the reason.

Nucleus Co-op launches a game through its **handler** for that game. Handlers
come from Nucleus Co-op's handler hub and are your own content: Rubylight
ships none. Open Nucleus Co-op, download the handler for each game you want to
play this way, and play it once there. A game without a handler is refused
with "No Nucleus Co-op handler for <exe>: add one in Nucleus Co-op's handler
hub, or use a child session".

Rubylight writes a Nucleus profile named **Rubylight** for each session and
loads it. When a start times out with "open Nucleus Co-op and load the profile
Rubylight once", do that on the PC and launch again.

The host must run as the Windows service (the installer's default), because
Nucleus Co-op runs elevated as the signed-in user. A portable host refuses
these launches and says why.

## Playing co-op

Each app whose game has a handler gets a second library entry, **Co-op
(Nucleus): <app>**. The first device that launches it starts a session with
one slot per player: the handler's maximum, capped at 4 and at
`nucleus_coop_players` (1 to 4, 4 by default). Every other device that
launches the same entry joins the running session in a free slot. Any
Moonlight client can join this way.

Joining a session that someone else started needs the device permission
**Join co-op** (`coop_join`, Experimental) on the device's page in the console.
Without it, the join is refused with "Turn on Join co-op for this device in the
Rubylight console." A device that already holds a slot can always go back to
it. When a device's stream ends, its slot is held for 120 s so it can come back.

In Nightjar the PC's details page lists the running co-op sessions on that PC
with their free slots and this device's own seat. **Join** (or **Return to my
slot**) takes the slot and starts the stream. A refusal is shown with the PC's
own words.

Maintenance in the console lists the sessions with their players. From there
you can remove a player or end a session. Ending a session stops Nucleus Co-op
and every instance, and ends each player's stream. A session also ends when
every player has left, when an instance crashes, or when its start fails; the
console and the log say which.

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

## Status

Covered by unit tests with fakes: install and hash checks, handlers and
profiles, the co-op session (slots, join, leave, the held slot, kick, crash,
end), controller routing, and Nightjar's list and join. The hardware runs (the
install, a background stream while the PC is in use, and a two-client co-op
session) are queued, and until they pass the feature stays Experimental. See
the [hardware matrix](hardware-matrix.md).
