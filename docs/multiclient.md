# Guests from other devices

Several people can play one game on the PC from their own paired devices. While one device plays (the **owner**), a second paired device that joins the running game becomes its **guest**. The guest sees the same picture and hears the same sound, gets a controller of its own as the next player, and gets the keyboard and mouse only if the owner grants them. Nightjar on Android and desktop and stock Moonlight clients can all join; the owner answers on their own device, or anyone with access answers in the console.

Guests are paired devices. To share a stream with someone in a browser without pairing, use [browser guests](browser-guests.md) instead.

## Joining

A device joins the running game in either of these ways:

- **Nightjar:** select the game another device is playing and press **Join** (shown as **Stream here too** on hosts without guests).
- **Moonlight and other clients:** start the app that is already running, or resume it. Both work as before when guests are off or when the device is the owner.

What happens next follows **When another device joins the running game** (`guest_join`, under **Settings → Games → Play together**):

| Value | What happens |
| --- | --- |
| **Ask the player** (`ask`) | The join waits up to 45 seconds while the owner's Nightjar and the console's Home page show the request. **Let in** starts the guest's stream; **Deny**, or no answer, refuses it with the reason. |
| **Let in without asking** (`allow`) | The device joins at once with **What a guest may do**. The owner can still change, kick or block it. |
| **Off** (`off`, default) | The device streams beside the game with its own device permissions, as before guests existed. One person moving between their own devices joins or resumes without anyone answering. |

A join is refused with a reason the client shows:

- "The player did not let you join"
- "Nobody answered your request to join"
- "This game has as many guests as it takes" (at most 4 guests at once)
- "The game ended before you could join"
- "The player does not let you join this game"

Moonlight clients show the text in their launch error.

Nightjar waits up to 120 seconds for a join answer. Moonlight clients that give up on a launch sooner than 45 seconds show their own timeout instead. In that case, answer sooner or switch to **Let in without asking**.

## What a guest may do

**What a guest may do** (`guest_permission`, default `controller`) is the permission a guest gets when the owner lets it in without choosing one. The owner can change it for each guest at any time.

| Permission | The guest gets |
| --- | --- |
| **View only** (`view`) | The picture and sound. Its controller, keyboard and mouse are ignored. |
| **Controller** (`controller`) | A virtual controller of its own, as the next player. Its keyboard and mouse are ignored. |
| **Controller, keyboard and mouse** (`controller-keyboard-mouse`) | Also the keyboard and mouse, one holder at a time (see below). It also gets the clipboard, USB forwarding and its microphone, where its own device permissions allow them. |

A guest's touch and pen input is always ignored: on the PC they act as a mouse. A guest never gets more than its own device permissions allow.

### One keyboard and mouse at a time

Only one person types and moves the pointer at a time:

- A guest with keyboard and mouse takes them by using them while nobody else has used them for two seconds.
- The owner takes them back by using its own keyboard or mouse.
- Someone at the PC takes them back by moving its mouse or pressing a key.
- The owner and the console can also hand them to a guest or take them back with **Give keyboard** and **Take back keyboard**.

When a guest loses the keyboard and mouse, the host releases any key or button it still held down. Controllers are not part of this rule: every controller plays at once.

The PC's own mouse and keyboard are watched only while a guest with keyboard and mouse is streaming.

## Kick, block and expiry

- **Kick** ends the guest's stream with "The player removed you from the game". The device may ask to join again.
- **Block** ends it with "The player does not let you join this game" and refuses that device's joins until the game ends.
- **How long a guest may stay** is chosen when letting it in: until the game ends (default), or 1 hour or 24 hours in the console. The host accepts 5 minutes to 7 days through the API. When the time is up the stream ends with "Your time as a guest in this game ended".
- When the game ends, or another game starts, every guest's stream ends with "The game you joined ended".

A guest that loses its connection keeps its place and can come back.

## Where to manage guests

- **The owner's Nightjar:**
  - when a device asks to join, Android asks during the stream with **Let in** and **Deny**; Nightjar for Windows and Linux shows a notice and answers under **Guests** in the stream menu's More page;
  - **Guests** in the stream menu, with each guest's permission, **Kick** and **Block**.
- **The console:**
  - on Home, a **Guests** card lists the joins waiting and the guests, with permission, stay, keyboard hand-over, **Kick** and **Block**;
  - each guest's stream card shows a **Guest** badge.
- **Another device with Host control** can manage guests through the native API, as the owner can.

## How the picture reaches each guest

Each guest gets **its own encoder on the owner's capture**. The guest's stream shares the virtual display the game is on, as **Stream here too** always has. It takes that display as it is and never changes the display's mode, HDR or scale. The capture is shared between streams of that display, so the GPU captures once. Each stream encodes it at its own codec, size, frame rate and bitrate.

We chose this over sending every viewer one shared encoded stream because:

- **The owner's latency does not change.** The owner's encoder, pacing and bitrate adaptation never wait for a guest or take its link into account. A guest on a slow network lowers only its own bitrate.
- **Every client keeps its own codec and size.** A phone can receive 720p H.264 while the owner plays 4K AV1. One shared stream would make every viewer accept the owner's codec and resolution, which stock Moonlight clients can't negotiate.
- **Loss recovery stays per device.** A guest's lost packets and reference-frame requests reach only its own encoder. With one shared stream, every viewer's loss would force keyframes on everyone.

The cost is one more encoder session per guest. A 4K guest takes about as much encoder time as a second 4K stream. A guest at a lower resolution takes proportionally less. The host allows at most 4 guests besides the owner, so the encoder sessions stay within what the drivers allow.

Sound reaches each guest like any second stream of the PC.

## Privacy

- A guest sees and hears everything the owner's stream shows, including notifications and other windows on that display. Let in only people you'd let watch your screen.
- The owner's device, the console and devices with Host control see each guest's and requester's device name, what it may do, and when it joined. Guests and other paired devices see the same list but can't change it.
- The host keeps the guest list in memory only. Nothing about guests is written to disk, except log lines that name the device and what happened (joined, let in, denied, kicked, blocked, ended).
- Watching the PC's local mouse and keyboard uses raw input only to see that someone at the PC used them. Nothing about what was typed or where the mouse moved is kept or sent.

See also [Privacy](privacy.md#your-own-devices).

## For developers

- **Host capability:** the native API lists the feature `session.guests` while `guest_join` is not `off`. Clients show Join, the approval prompt and the guest list only when it is listed. The host keeps `PROTOCOL_VERSION`, and Moonlight clients use no new messages: a guest's stream is an ordinary stream.
- **Endpoints:** the routes and types are in [Native API: Guests from other devices](native-api.md#guests-from-other-devices-sessionguests). The console's equivalents are under `/api/session-guests`.
- **Code:**
  - The join policy, the guest list and the keyboard-and-mouse holder are pure state machines in `rust/core/src/multiclient.rs`, tested in `rust/core/src/multiclient/tests.rs`.
  - The host side is in `rust/host/src/multiclient.rs`, with its join gate in `nvhttp.rs` and input gating in `stream/control.rs`.
  - The PC's local input watcher is in `rust/windows/src/local_input.rs`.
  - The launch option `rlGuest` marks a guest's launch. The host sets it and removes it from every client request.
