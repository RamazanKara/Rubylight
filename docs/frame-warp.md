# Frame Warp

Frame Warp makes a camera turn show on the next refresh of the client's screen instead of a
round trip and the game's own delay later. Nightjar shifts each picture by the turn that its
own mouse or right-stick input has not reached the picture with yet. Rubylight learns, for the
running game, how far the picture moves per mouse count and per second of stick, how long the
game takes to show an input and, for a game drawn through a perspective camera, its field of
view, and tags every frame with how much input it reflects. What stays put on screen while the
camera turns (crosshair, health, minimap, the weapon) is found by the host and left unshifted.

On a test game with 50 ms of delay it removed 96% of the picture's trail behind the mouse, and at
a 90 degree field of view 91% at the picture's edge; the host's GPU passes take 0.14 to 0.16 ms a
frame from 1080p to 4K on the RX 7900 XT ([Measuring](#measuring)).

## Turning it on

It is a setting in Nightjar (Video, **Frame Warp**), off by default, and needs a host that
lists it (`FrameWarpVersion` in serverinfo). The host's `frame_warp` setting (**All settings ›
Picture › Frame pacing › Let devices use Frame Warp**, on by default) lets clients use it; the
host does nothing for Frame Warp until a client asks.

What a stream learned is kept for the next one: the host saves each game's gains, lags, stick
curve and field of view in `frame_warp.json` in its configuration directory, by the app's uuid
or else its name, for the 256 games streamed most recently. A later stream of that game applies
the lags at once and locks the gains as soon as recent input confirms them.

## How it works

1. [The client asks](#the-request) for Frame Warp once the stream runs.
2. [The host records the input](#input-history) it receives.
3. [It measures how far each captured picture moved](#measuring-motion).
4. [It fits that motion to the input](#gain-and-lag): a gain per axis and the game's lag.
5. [It learns the game's field of view](#field-of-view) from how the picture moves in a turn.
6. [It tags every frame](#tags-and-model) with the input it reflects, and sends the model when it
   changes.
7. [It finds the HUD](#the-hud-mask) and sends the client a mask of it.
8. [The client shifts each picture](#drawing-the-shift) by the input the picture does not show
   yet.

### The request

The client sends a request (0x5543) once the stream runs: on, the controller number whose right
stick it tracks, and the widest HUD mask it takes.

### Input history

The host keeps the running sums of the relative mouse motion it received, and for the tracked pad
the count of controller packets it received and their right stick, with the time each arrived.

### Measuring motion

For each capture the GPU makes a 256-pixel-wide luma thumbnail; the CPU measures how far the
content moved from the previous one (`rust/core/src/frame_warp/motion.rs`): row and column
profiles of a 3×3 grid of cells are correlated, refined to a tenth of a pixel by gradient steps,
and the median cell wins.

The HUD found so far ([the HUD mask](#the-hud-mask)) is left out of the profiles, since a wide
static bar would pull every vertical shift toward zero, and a picture whose textured cells are
mostly unchanged counts as standing still whatever moves in a corner of it. That mask is the one
the client holds still, so it leaves out a HUD's rim and its plain parts, and a client may ask for
none. The measurement therefore also finds what stands still itself, per thumbnail pixel: each
move should bring a pixel what its cell's shift says lay that far away, and a pixel that should
have changed by 24 luma codes in all (at most 6 from one move) and stayed within 2 is left out
with its neighbours until it changes. A sky, a horizon during a pan or the middle of a plain
panel, which a move brings more of the same, is not marked.

A profile takes the moving average out of every picture line before it averages the lines, and
drops the samples whose average would reach over a left-out pixel: otherwise rows half under a
HUD stand at another level than their neighbours, and that step, fixed on the screen, holds the
cell's shift back as the HUD itself would.

### Gain and lag

The calibrator (`calibrate.rs`) fits the moves to the input over the last four seconds: for every
lag from 0 to 200 ms, a least-squares gain per axis, and the lag that explains the most motion
wins. An axis with less than a quarter of the other's input takes the other's gain: fitted to the
slight drift of a sideways turn, it would mostly fit whatever else moves the picture that way, a
ship's roll.

It locks at 80% explained and unlocks when the last 1.2 s of input with the locked model explain
less than 50% (a menu, a cutscene, a cursor). While locked, a new fit replaces the model only
when it explains at least 3 points more of the motion, or a quarter of what the model leaves
unexplained when that is less, so the lag doesn't wander from fit to fit; otherwise the model
keeps its lag and takes the least-squares gain there, which settles as turns add up after an
early lock.

### Field of view

A game draws through a perspective camera, so a turn moves the edges of its picture farther than
the centre, twice as far at the left and right of a 90 degree field of view. The host learns the
field of view from the cells (`lens.rs`): for each move made while the player's turning was
arriving, and for each field of view from 30 to 120 degrees in steps of 2, it fits the cells'
shifts with a camera turn, a zoom about the centre (walking forward) and a roll, leaving out up
to two cells where something else moves, and keeps the squared remainder, fading over the last
few seconds of turning.

Each shift counts where its profile measured it rather than at the cell's centre: a profile
follows its slopes, a lens stretches the picture toward its edge, so an outer cell's shift is
that of a place nearer the middle (enough to read 90 degrees as 78), and a cell half under a HUD
is measured beside it. No cell costs a field of view more than a cell left out does, so a cut or
a menu opening costs them all alike.

Once 30 moves have added up, the field of view that leaves the least (between the steps, where a
parabola through the best and its neighbours is lowest) is taken if it leaves at most 75% of what
a flat picture leaves, changed for one at least 1.5 degrees away that leaves at most 98% of what
it leaves, and dropped when it leaves more than 90% of the flat picture's remainder.

With a lens, each move is read as the turn at the picture's centre from that fit, not the median
cell, so the gains are the centre's; a map or a side-scroller, which moves alike everywhere, gets
no lens and stays as before. A client whose request does not say it draws the lens (Nightjar 5.0)
is sent no focal length, and the centre's gains all the same: the median cell's, longer, would
carry the middle of its picture too far.

### Tags and model

Each frame, as its number is assigned, is tagged with the mouse sums and the pad packet count as
of its capture time less the game's lag, and how long that packet had been held (0x5545, with the
three previous frames' tags, unreliable).

The model (0x5544, reliable) goes out when it locks, unlocks, a gain changes by more than 2%, the
lag by more than 3 ms or the lens's focal length by more than 3%.

### The HUD mask

While the camera turns, the GPU compares every pixel with the same pixel in an earlier capture,
within 2/255 per channel, since games add faint noise or dither to their HUD. The earlier capture
is one the scene has moved at least a thumbnail pixel since: the previous one in a fast turn, a
few back in a slow turn at a high frame rate, which would otherwise move too little between
captures for a textured scene to change. A pixel whose value changed still counts as staying when
it differs from its right and lower neighbours as it did: the edge of a translucent HUD, grey text
over the sea, keeps its place and its contrast while its value follows the scene behind it.

A mask cell gains a point when its textured pixels (those that differ from a neighbour by more
than the tolerance) mostly stayed, and loses half when they or a quarter of all its pixels
changed. A smooth cell, a sky or fog, looks the same moved or not and keeps its score; with the
tolerance it would otherwise become HUD and hide whatever crosses it. A cell that held through
about 24 comparisons is HUD. The host joins HUD pieces up to 3% of the picture's height apart
into one region, so text found letter by letter stays in place as a whole, and grows the mask by
a cell for anti-aliased edges.

The mask is sent at the client's width or less, at most once a second when it changes (0x5546,
reliable, run-length coded). The client lays it over the whole picture: a capture of another
shape than the stream, letterboxed into it, has its grid placed where it shows, and the bars count
as HUD, so they hold still and nothing is shifted in from them. Thumbnails and gains likewise
cover only the capture's part of the picture.

### Drawing the shift

The client keeps its own sums of what it sent. For each picture it draws, as late as it can, the
shift is

```text
shift = mouse_gain * (mouse_sent - tag.mouse)
      + stick_gain * integral of curve(stick sent) from tau to now
```

where `tau` is the send time of the tagged pad packet plus how long the host had held it, clamped
to 15% of the picture (less under a lens wider than 90 degrees, whose edges stretch sooner).

The client samples the picture at `position - shift`, or, when the model has a lens, where a
camera turned by `shift / focal` shows that position: the centre moves by
`focal * tan(shift / focal)` and the edges by more, as the game itself would draw the turn
([the mapping](../client/docs/protocol/frame-warp.md#the-shift)). It samples at `position` where
the HUD mask is set or where the source lands on it, so the HUD stays in place without a second
copy beside it. The strip the shift uncovers mirrors the picture back from its edge, darkened by
up to 35% over the first 8% of the picture, so it reads as an edge rather than a smear of the
outermost pixels.

The same shift is drawn again on refreshes between pictures, so a 60 fps stream turns smoothly
on a 120 Hz screen.

## Accuracy limits

- The shift is a camera turn: right all over the picture for a turn left or right, or up or
  down, once the field of view is learned (within 3.5 degrees from some seven seconds of
  turning in our runs; the first seconds use a narrower one or none). A turn left or right while
  the camera looks up or down also rolls the picture, which the shift leaves out: at 10 degrees of
  pitch that is most of what remains at the picture's edges. Movement (strafing) is not predicted.
- A client that does not draw the lens slides the picture by the centre's turn: right in the
  middle, too short toward the edges.
- The field of view is learned from the player's own turns, so a game that is only walked
  through, or whose picture a HUD covers in most cells, keeps the flat shift.
- A model fits the game's sensitivity as it is; in-game sensitivity changes refit within about
  four seconds. Mouse acceleration in the game makes the fit worse and may keep it unlocked.
- A HUD element that moves or fades, or a scene that changes nowhere, can leave HUD shifted or
  scene unshifted for a moment. A HUD so faint that the scene behind it shows through more than
  it does is found only in part, and the scene between HUD pieces joined into one region
  stays in place with them.
- The client's and host's sums start at the request that starts Frame Warp, and a client that
  has sent no input for 600 ms sets them equal again, so a lost count lasts until the next
  pause.

## Measuring

`rust/windows/examples/look_probe.rs` is a fake mouse-look game whose response is known exactly.
Run it on a display an isolated test host captures (`--injected-only`, so only the host's moves
turn it), and on the same PC stream that display at its own size with nightjar-cli, setting
`NIGHTJAR_CLI_FRAME_WARP=<csv>` (ask for Frame Warp and log the shift each decoded picture gets)
and `NIGHTJAR_CLI_LOOK=<seed>` (a mouse-look pattern of turns and pauses). The probe's barcode
ties each logged picture to the input it shows, and `rust/release/frame_warp_accuracy.py --probe
OUT.json --client <csv>` compares each shift with the ideal one: the learned gain and lag, how
soon the model locks, and how much of the input-to-picture displacement Frame Warp removes.
With `--fov DEGREES` the probe draws its world through a perspective camera, and the script
also reports the learned field of view and what is left at the centre, over the whole picture
and at its edge (a 7 by 7 grid of points, each compared with where the turned camera shows it).

The results, from [Performance](../rust/PERFORMANCE.md#november-12-frame-warp) and
[the lens's](../rust/PERFORMANCE.md#november-12-frame-warp-lens) runs (2560x1440, 60 fps):

| Run | Without Frame Warp | Left with Frame Warp | Removed |
|---|---|---|---|
| Flat picture, 50 ms of game delay (median distance while turning) | 64.9 px | 1.4 to 2.2 px | 95.8 to 96.4% |
| 90 degree field of view, centre (median) | 67.7 px | 2.2 px | 96.0% |
| 90 degree field of view, whole picture | 91.9 px | 8.0 px | 91.8% |
| 90 degree field of view, edge | 105.0 px | 10.2 px | 90.8% |
| 90 degree field of view, edge, a host and client without the lens (Rubylight 4.0, Nightjar 5.0) | 104.2 px | 36.5 px | 65.4% |

The field of view was learned as 88.9 degrees of 90, 67.8 of 70 and 107.6 of 110.

## Wire formats

`client/protocol/core/src/frame_warp.rs` defines every message; [control messages](control-messages.md)
lists the ids, and Nightjar's [protocol document](../client/docs/protocol/frame-warp.md) gives the
full rules for other hosts. Little-endian throughout.

| Message | Bytes |
|---|---|
| Request 0x5543 | version 1, enabled 0/1, controller 0-15 or 0xFF, flags (bit 0: the client draws the lens), mask max width u16, 0, 0 |
| Model 0x5544 | version, flags (bit 0 mouse locked, bit 1 stick locked), generation u16, picture width u16, height u16, then f32: mouse gain x, y (picture px per count), stick gain x, y (px per second of full deflection after the curve), stick dead zone, stick exponent (1 to 3 in halves), max shift x, y; lag µs u32; fit f32; focal length f32 (picture px, 0 for none; absent from hosts before it was added, and clients from before it ignore it) |
| Tags 0x5545 | version, count 1-4, model generation u16, then per tag: frame u32, mouse sum x i32, y i32, pad packet count u32, held µs u32 (0xFFFFFFFF none) |
| Mask 0x5546 | version, kind (0 none, 1 runs), generation u16, width u16, height u16, then per row LEB128 run lengths alternating clear and set, starting clear |

## Where the code is

| Part | Code |
|---|---|
| Wire formats, stick curve | `client/protocol/core/src/frame_warp.rs` |
| Host history, motion, lens, calibration, tags, mask choice | `rust/core/src/frame_warp/` |
| Host GPU thumbnail and HUD passes | `rust/windows/src/compute_frame_warp.rs`, `rust/windows/src/shaders/color.hlsl` |
| Host wiring, setting and the per-game memory | `rust/host/src/stream/frame_warp.rs`, `rust/core/src/config/overrides.rs` |
| Client counting and shift | `client/protocol/client/src/frame_warp.rs`, `input/queue.rs`, `control/session.rs` |
| Desktop presenters | `client/protocol/desktop/src/stream/` |
| Android renderers | `client/app/src/main/jni/vulkan/`, `GlesUpscaler.java`, `pyrowave-renderer/` |
