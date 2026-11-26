# Frame Warp

[Nightjar docs](../index.md) › [Engineering notes](../index.md#engineering-notes) · [Host feature support](../PARITY-MATRIX.md) · [Video and latency](../video-and-latency.md)

Status: version 2 (Frame Warp 2), which replaces version 1: neither end speaks version 1 any more. Nightjar implements the client side, and Rubylight the host side. Any host can implement it; a client checks the advertisement, never the host name.

A camera turn reaches the screen a network round trip plus the game's own delay after the input. Frame Warp lets the client show it on its next refresh: it shifts each picture by the turn that its own mouse or right-stick input has not reached that picture with yet, and leaves what stays put on screen (crosshair, health, minimap, the weapon) where it is. The host does the learning: per game, how far the picture moves per mouse count and per second of stick, how long the game takes to show an input, and where the HUD is. It tags each frame with how much input the frame reflects. The client only adds up what it sent.

The shift is a translation. It is right for a camera turn near the screen's centre, approximate near its edges at wide fields of view, and does not predict movement such as strafing.

## Overview

1. The host advertises the extension in its serverinfo reply and in the RTSP DESCRIBE answer of each stream.
2. The client sends a **request** (`0x5543`) on the encrypted control stream once the stream runs.
3. The host learns a **model** of the running game and sends it (`0x5544`) when it locks, unlocks or changes.
4. Each video frame, as its number is assigned, gets a **tag** (`0x5545`): the input it reflects.
5. While the camera turns the host finds what stays put and sends a **HUD mask** (`0x5546`) when it changes.
6. With each frame's tags goes a **meta** (`0x5549`): how sure the host is that the camera moves the picture, the gyro input the frame reflects, and how to fill the strip a shift uncovers.
7. The client computes a shift for every picture it draws and samples the picture at `position - shift`, except inside the mask.

## Advertising support

**serverinfo** (`/serverinfo`, HTTP and HTTPS):

```xml
<FrameWarpProtocol>2</FrameWarpProtocol>
```

The version of this document the host speaks, as a decimal integer. A missing field, or anything that is not a decimal number from 2 to 65535, means no support. Hosts before Rubylight 7.0 listed `<FrameWarpVersion>1</FrameWarpVersion>` instead, for version 1, which took an 8-byte request; a current host does not list it, so an older client sees no Frame Warp, and a current client that sees only `FrameWarpVersion` tells the player that Frame Warp needs the current version on the PC and sends nothing.

**Per stream** (RTSP DESCRIBE answer, the line hosts use for host-to-client control message ids):

```text
a=x-rl-control:... 0x5543 0x5544 0x5545 0x5546 0x5549
```

The ids may share the line with others. The client sends a request only when both are present, `0x5549` included (a version 1 host listed `0x5543` to `0x5546` only). A host that turns the feature off for a stream leaves the ids out.

## The shift

The client keeps, from the request that started Frame Warp:

- the running sums of the relative mouse motion it sent (`MouseMove` dx and dy, as sent on the wire, after any client-side scaling), and
- for the tracked controller, the count of controller packets it sent, the send time of each and the right stick in each.

For a picture whose frame carries tag `T` under model `M`, drawn at time `now`:

```text
mouse = M.mouse_gain * (mouse_sum_sent - T.mouse)                       (per axis)
tau   = send_time(packet T.controller_seq) + T.controller_held_us
stick = M.stick_gain * integral from tau to now of curve(stick sent)  (per axis)
shift = clamp(mouse if M.mouse_locked + stick if M.stick_locked, -M.max_offset, +M.max_offset)
```

Shifts are in picture pixels (the stream's video size, `M.picture_width` by `M.picture_height`) and move the content: the client samples the picture at `position - shift`. Mouse y grows downward as in `MouseMove`; the stick's y is the input stream's (up is positive), and the model's sign carries the direction. `curve` is the stick response below. The sums wrap at 2³²; take differences as signed 32-bit values. How the client fills the strip that the shift uncovers is its own choice: Nightjar mirrors `position - shift` back into the picture and darkens the fill by up to 35% over its first 8% of the picture past the edge. Where `position - shift` lands on the HUD mask, Nightjar samples `position` itself, so the HUD isn't drawn a second time beside itself.

**The lens.** A game draws through a perspective camera, so a turn moves the edges of its picture farther than the centre: twice as far at the left and right edges of a 90 degree field of view. A model may therefore carry the camera's focal length `M.focal` in picture pixels (`M.picture_width / 2 / tan(horizontal field of view / 2)`), and its gains are then those of the picture's centre. `shift / M.focal` is the angle the camera turned by, per axis, and the client draws the picture turned by it. With `t = tan(shift / M.focal)` per axis, `size` the picture's size, and positions `p` as fractions of the picture with `q = p - 0.5`:

```text
centre = M.focal * t                                  (how far the picture's centre moves, in pixels)
k      = t * size / M.focal
n      = (sqrt(1 + t.y²), sqrt(1 + t.x²))
source = 0.5 + (q * n - centre / size) / max(1 + dot(q, k), 0.25)
```

and the client samples the picture at `source` instead of `position - shift`. This is exact for a turn about one axis and within the product of the two tangents for both at once (under a pixel at the corners of a shift of 3% of the picture each way). A model without a focal length (0, or a 48-byte model) describes a picture that moves alike everywhere, and `source` is then `p - shift / size`. The edge fill and the HUD mask apply to `source` as they do to `position - shift`. A client asks for the focal length with the request's lens flag; one that does not is sent none.

The client should compute the shift as late as it can before each present, and draw the same picture again with a new shift on refreshes between pictures. When nothing has been sent for 600 ms, both sums are equal to the host's (no input is in flight), so a lost count lasts until the next pause; Nightjar uses this to rebase. When the model unlocks or the client stops, the shift fades to zero over about 150 ms rather than jumping.

**Stick response** (both sides integrate the same function): with `vx = x / 32767` and `vy = y / 32767` clamped to -1..1, `r = sqrt(vx² + vy²)`; zero when `r` is at most the dead zone, otherwise `s = min((r - deadzone) / (1 - deadzone), 1)` and the result is `(vx / r, vy / r) * s^exponent`.

## Messages

All messages travel on the encrypted control stream (`SS_ENC_CONTROL_V2`). All integers are little-endian; floats are IEEE 754 binary32. A receiver drops a message that breaks any rule below. Every message may grow in a later version: a receiver reads the fields it knows and ignores trailing bytes.

### `0x5543` request, client to host (12 bytes, reliable)

| Offset | Size | Field | Rule |
| --- | --- | --- | --- |
| 0 | u8 | version | 2. A host ignores anything else, and anything shorter than 12 bytes (an older client's 8-byte version 1 request), and sends nothing for it |
| 1 | u8 | enabled | 1 = learn, tag and mask; 0 = stop all three |
| 2 | u8 | controller | Input stream controller number (0 to 15) whose right stick turns the camera; 0xFF for none |
| 3 | u8 | flags | Bit 0: the client draws the picture through the model's lens ("The lens" above); bit 1: the gyro turns the camera too. Other bits 0; a host ignores them |
| 4 | u16 | mask max width | Widest HUD mask the client takes, at most 2048; 0 asks for none |
| 6 | u16 | reserved | 0 |
| 8 | u8 | gyro controller | Controller number (0 to 15) whose motion packets carry the gyro, or 0xFF for the stick's controller |
| 9 | u8 | gyro axes | Bit 0: invert yaw; bit 1: invert pitch; bits 2-3: yaw from the motion packet's y axis (0), its z axis (1) or as the host finds it (2); 3 rejects; bits 4-7 0, ignored |
| 10 | u16 | max offset | The client's limit of the shift in thousandths of the picture, at most 1000; 0 for the host's 15% |

An enable request that starts Frame Warp (the first, or the first after a disable) restarts the counts on both sides: the host counts the input it receives from then on, the client the input it sends from then on. Input and control messages travel on different channels, so the counts can still differ by what was in flight; the idle rule above settles that. A repeated enable request keeps the counts. Every enable request makes the host send its current model and mask again. A host stops when the stream ends.

### `0x5544` model, host to client (52 bytes, reliable)

| Offset | Size | Field | Rule |
| --- | --- | --- | --- |
| 0 | u8 | version | 1 |
| 1 | u8 | flags | Bit 0 mouse locked, bit 1 stick locked; other bits 0 |
| 2 | u16 | generation | One more than the previous model (wrapping) |
| 4 | u16 | picture width | Greater than 0 |
| 6 | u16 | picture height | Greater than 0 |
| 8 | f32 × 2 | mouse gain x, y | Picture pixels per mouse count, signed, finite, magnitude at most 100000 |
| 16 | f32 × 2 | stick gain x, y | Picture pixels per second of full deflection after the curve, same bounds |
| 24 | f32 | stick dead zone | 0 to 0.5 |
| 28 | f32 | stick exponent | 1, 1.5, 2, 2.5 or 3 |
| 32 | f32 × 2 | max shift x, y | 0 to 65535 picture pixels |
| 40 | u32 | lag µs | The game's lag the tags allow for (for statistics) |
| 44 | f32 | fit | 0 to 1 (for statistics) |
| 48 | f32 | focal | The game camera's focal length in picture pixels, 1 to 1000000, or 0 for a picture that moves alike everywhere ("The lens" above). The gains are then the picture centre's |

A host sends a focal length only to a client whose request set the lens flag; any other client gets 0. Its gains are the picture centre's as well, so drawn at `position - shift` its picture is right in the middle and moves too little toward the edges. A model of 48 bytes, as hosts sent before the focal length was added, has none. A host limits `max shift` further under a lens, so that the turn never brings the picture's edge near level with the camera.

The host sends a model when it locks or unlocks, and when a locked gain changes by more than a few percent or the focal length does. Neither part locked means: draw unshifted.

### `0x5545` tags, host to client (4 + 20 × count bytes, unreliable)

| Offset | Size | Field | Rule |
| --- | --- | --- | --- |
| 0 | u8 | version | 1 |
| 1 | u8 | count | 1 to 4 |
| 2 | u16 | model generation | The model in force when the newest frame was tagged |

Then `count` tags, oldest first:

| Offset | Size | Field | Rule |
| --- | --- | --- | --- |
| 0 | u32 | frame | The video frame number of the RTP/NV header |
| 4 | i32 | mouse x | The host's running sum of received mouse dx as of the frame's capture time less the game's lag, modulo 2³² |
| 8 | i32 | mouse y | The same for dy |
| 12 | u32 | controller packets | Packets received from the tracked controller by then, counted from the request |
| 16 | u32 | held µs | How long the last of those packets had been held by then; 0xFFFFFFFF when none had arrived |

The host sends one message per frame as the frame goes out, carrying that frame's tag and the previous three, so one lost message costs nothing. A client that has no tag for a frame may extrapolate from the two before it for a few frames.

### `0x5546` HUD mask, host to client (8 bytes + runs, reliable)

| Offset | Size | Field | Rule |
| --- | --- | --- | --- |
| 0 | u8 | version | 1 |
| 1 | u8 | kind | 0 = no mask (width and height 0), 1 = runs |
| 2 | u16 | generation | One more than the previous mask (wrapping) |
| 4 | u16 | width | Cells across, at most the request's mask max width and 2048 |
| 6 | u16 | height | Cells down, at most 2048 |
| 8 | ... | runs | Per row, top to bottom: LEB128 run lengths alternating clear and set, starting with clear (a row that starts set begins with 0). A row's runs add up to `width`; no zero run except a row's first |

The mask covers the whole picture; a set cell stays unshifted. A message is at most 48 KiB; a host halves the mask's resolution (a cell set when any of the four it covers is) until it fits. Hosts send masks at most about once a second, only when enough cells changed.

### `0x5549` meta, host to client (40 bytes, unreliable)

One per tagged frame, after its tags: the gyro as a third camera input, a confidence that fades the shift out when a menu, a cutscene or a pointer moves the picture, and hints for drawing the strip a shift uncovers and the edges of the HUD.

| Offset | Size | Field | Rule |
| --- | --- | --- | --- |
| 0 | u8 | version | 2 (0 rejects) |
| 1 | u8 | flags | Bit 0 gyro gain locked; bit 1 the picture just cut to another scene; bit 2 a pointer shows on the host; bit 3 soft HUD edges; bit 4 locked from the game's saved prior; bit 5 the gyro's yaw is the z axis (the host's choice for yaw axes 2). Unknown bits are ignored |
| 2 | u16 | model generation | As the tags' |
| 4 | u32 | frame | The video frame number, as the tags' |
| 8 | u16 | camera confidence | 0 to 65535 for 0 to 1: how sure the host is that the camera moves the picture |
| 10 | u16 | HUD stability | 0 to 65535 for 0 to 1: how steady the HUD mask is |
| 12 | u32 | gyro seq | Motion packets with a gyro reading received from the gyro controller (wrapping) as of the frame's reflection time; 0 for none |
| 16 | u32 | gyro held | How long the last of them had been held then, µs; 0xFFFFFFFF for none |
| 20 | f32 × 2 | gyro gain | Picture centre pixels per radian of yaw and of pitch; finite, at most 100000 in magnitude |
| 28 | i32 × 2 | omega | The camera's yaw and pitch rate the host measured, milliradians per second |
| 36 | u8 | edge fade | How dark the far edge of the uncovered strip gets, 0 to 255 for none to black |
| 37 | u8 | HUD guard | Mask cells to probe back along the shift for a background pixel, 0 to 8 (more rejects) |
| 38 | u16 | reserved | 0, ignored |

The gyro rate of a motion packet (degrees per second: x pitch, y yaw, z roll) is `(yaw, pitch)` in radians per second, with the yaw from y or z and each axis inverted as the request says. Both ends learn the gyro's zero-rate bias alike from the packets: a reading under 0.05 rad/s on both axes for 20 packets in a row moves the bias 2% of the way to it (`GyroBias` in `rubylight_protocol::frame_warp_v2`). The shift gains

```text
gyro_gain * integral(rate - bias, tau_g..now)
```

with `tau_g` the client's send time of gyro packet `gyro seq` plus `gyro held`, and the client scales the whole shift by the confidence: down with a 40 ms time constant, back up along a 180 ms smoothstep (full confidence before the first meta). Motion packets travel unreliably, so the client sets its gyro count to the host's whenever its gyro has been still for 600 ms. A point past the picture's edge samples back inside by a quarter of how far past it is and is darkened by up to `edge fade / 255` along a smoothstep over 8% of the picture (`edge fade` 90 before the first meta); a point whose source lands on the HUD tries up to `HUD guard` points (2 before the first meta) one mask cell apart back along the shift before it keeps its own pixel.

## Client behavior (Nightjar)

- **Frame Warp** is a Video setting, off by default. Nightjar sends a request only when the host advertises both the serverinfo field and the ids for the stream, and not for a remote monitor stream.
- It counts input at the point it goes on the wire, so what it adds up is exactly what the host received.
- Tags are parsed on the control thread; models and masks are checked there and applied atomically between pictures. Video decoding is untouched: the shift happens where the picture is drawn.
- The overlay shows the current shift, whether the model is locked, and how many pictures were shifted, extrapolated or untagged.
- Every renderer draws the lens mapping above per pixel in its shader; the desktop's SDL renderer path draws it as triangles small enough to stay within a fifth of a pixel of it at 4K.
- A host that lists only `FrameWarpVersion` (before Rubylight 7.0) gets no request; with the setting on, Nightjar says that Frame Warp needs the current version on the PC and streams without it.

## Host requirements

- Count mouse motion and controller packets as they are received, before injection, in the same order the client sent them.
- Send nothing before an enable request, and stop when a disable request or the end of the stream arrives.
- Never send a mask wider than the request's mask max width, or a model whose values break the bounds above.
- Ignore a request that is not version 2 or is shorter than 12 bytes, and send no Frame Warp message for it: the stream goes on without Frame Warp.
- Send a meta (`0x5549`) after the tags of every tagged frame.
