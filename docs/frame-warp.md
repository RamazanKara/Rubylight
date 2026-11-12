# Frame Warp

Frame Warp makes a camera turn show on the next refresh of the client's screen instead of a
round trip and the game's own delay later. Nightjar shifts each picture by the turn that its
own mouse or right-stick input has not reached the picture with yet. Rubylight learns, for the
running game, how far the picture moves per mouse count and per second of stick, and how long
the game takes to show an input, and tags every frame with how much input it reflects. What
stays put on screen while the camera turns (crosshair, health, minimap, the weapon) is found by
the host and left unshifted.

It is a setting in Nightjar (Video, **Frame Warp**), off by default, and needs a host that
lists it (`FrameWarpVersion` in serverinfo). The host's `frame_warp` setting (on by default)
lets clients use it; the host does nothing for Frame Warp until a client asks.

## How it works

1. The client sends a request (0x5543) once the stream runs: on, the controller number whose
   right stick it tracks, and the widest HUD mask it takes.
2. The host keeps the running sums of the relative mouse motion it received, and for the
   tracked pad the count of controller packets it received and their right stick, with the
   time each arrived.
3. For each capture the GPU makes a 256-pixel-wide luma thumbnail; the CPU measures how far the
   content moved from the previous one (`rust/core/src/frame_warp/motion.rs`): row and column
   profiles of a 3×3 grid of cells are correlated, refined to a tenth of a pixel by gradient
   steps, and the median cell wins. The HUD found so far (step 6) is left out of the profiles,
   since a wide static bar would pull every vertical shift toward zero, and a picture whose
   textured cells are mostly unchanged counts as standing still whatever moves in a corner of
   it. The calibrator
   (`calibrate.rs`) fits the moves to the input over the last four seconds: for every lag from
   0 to 200 ms, a least-squares gain per axis, and the lag that explains the most motion wins.
   An axis with less than a quarter of the other's input takes the other's gain: fitted to the
   slight drift of a sideways turn, it would mostly fit whatever else moves the picture that way,
   a ship's roll. It locks at 80% explained and unlocks when the last 1.2 s of input with the
   locked model explain less than 50% (a menu, a cutscene, a cursor). While locked, a new fit
   replaces the model only when it explains at least 3 points more of the motion, or a quarter
   of what the model leaves unexplained when that is less, so the lag doesn't wander from fit
   to fit; otherwise the model keeps its lag and takes the least-squares gain there, which
   settles as turns add up after an early lock.
4. Each frame, as its number is assigned, is tagged with the mouse sums and the pad packet
   count as of its capture time less the game's lag, and how long that packet had been held
   (0x5545, with the three previous frames' tags, unreliable).
5. The model (0x5544, reliable) goes out when it locks, unlocks, a gain changes by more than 2%
   or the lag by more than 3 ms.
6. While the camera turns, the GPU compares every pixel with the same pixel in an earlier
   capture, within 2/255 per channel, since games add faint noise or dither to their HUD. The
   earlier capture is one the scene has moved at least a thumbnail pixel since: the previous one
   in a fast turn, a few back in a slow turn at a high frame rate, which would otherwise move
   too little between captures for a textured scene to change. A pixel whose value changed
   still counts as staying when it differs from its right and lower neighbours as it did: the
   edge of a translucent HUD, grey text over the sea, keeps its place and its contrast while
   its value follows the scene behind it. A mask cell gains a point when its textured pixels
   (those that differ from a neighbour by more than the tolerance) mostly stayed, and loses
   half when they or a quarter of all its pixels changed. A smooth cell, a sky or fog, looks
   the same moved or not and keeps its score; with the tolerance it would otherwise become HUD
   and hide whatever crosses it. A cell that held through about 24 comparisons is HUD. The host
   joins HUD pieces up to 3% of the picture's height apart into one region, so text found
   letter by letter stays in place as a whole, and grows the mask by a cell for anti-aliased
   edges. The mask is sent at the client's width or less, at most once a second when it
   changes (0x5546, reliable, run-length coded). The client lays it over the whole picture: a
   capture of another shape than the stream, letterboxed into it, has its grid placed where it
   shows, and the bars count as HUD, so they hold still and nothing is shifted in from them.
   Thumbnails and gains likewise cover only the capture's part of the picture.
7. The client keeps its own sums of what it sent. For each picture it draws, as late as it
   can, the shift is

   ```text
   shift = mouse_gain * (mouse_sent - tag.mouse)
         + stick_gain * integral of curve(stick sent) from tau to now
   ```

   where `tau` is the send time of the tagged pad packet plus how long the host had held it,
   clamped to 15% of the picture. The client samples the picture at `position - shift`, and at
   `position` where the HUD mask is set or where `position - shift` lands on it, so the HUD
   stays in place without a second copy beside it. The strip the shift uncovers mirrors the
   picture back from its edge, darkened by up to 35% over the first 8% of the picture, so it
   reads as an edge rather than a smear of the outermost pixels.

The same shift is drawn again on refreshes between pictures, so a 60 fps stream turns smoothly
on a 120 Hz screen.

## Accuracy limits

- The shift is a translation: right for a camera turn near the screen's centre, approximate
  near its edges at wide fields of view, and wrong for movement (strafing), which it does not
  try to predict.
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
[Performance](../rust/PERFORMANCE.md#november-12-frame-warp) has the results.

## Wire formats

`client/protocol/core/src/frame_warp.rs` defines every message; [control messages](control-messages.md)
lists the ids, and Nightjar's [protocol document](../client/docs/protocol/frame-warp.md) gives the
full rules for other hosts. Little-endian throughout.

| Message | Bytes |
|---|---|
| Request 0x5543 | version 1, enabled 0/1, controller 0-15 or 0xFF, 0, mask max width u16, 0, 0 |
| Model 0x5544 | version, flags (bit 0 mouse locked, bit 1 stick locked), generation u16, picture width u16, height u16, then f32: mouse gain x, y (picture px per count), stick gain x, y (px per second of full deflection after the curve), stick dead zone, stick exponent (1 to 3 in halves), max shift x, y; lag µs u32; fit f32 |
| Tags 0x5545 | version, count 1-4, model generation u16, then per tag: frame u32, mouse sum x i32, y i32, pad packet count u32, held µs u32 (0xFFFFFFFF none) |
| Mask 0x5546 | version, kind (0 none, 1 runs), generation u16, width u16, height u16, then per row LEB128 run lengths alternating clear and set, starting clear |

## Where the code is

| Part | Code |
|---|---|
| Wire formats, stick curve | `client/protocol/core/src/frame_warp.rs` |
| Host history, motion, calibration, tags, mask choice | `rust/core/src/frame_warp/` |
| Host GPU thumbnail and HUD passes | `rust/windows/src/compute_frame_warp.rs`, `rust/windows/src/shaders/color.hlsl` |
| Host wiring and setting | `rust/host/src/stream/frame_warp.rs`, `rust/core/src/config/overrides.rs` |
| Client counting and shift | `client/protocol/client/src/frame_warp.rs`, `input/queue.rs`, `control/session.rs` |
| Desktop presenters | `client/protocol/desktop/src/stream/` |
| Android renderers | `client/app/src/main/jni/vulkan/`, `GlesUpscaler.java`, `pyrowave-renderer/` |
