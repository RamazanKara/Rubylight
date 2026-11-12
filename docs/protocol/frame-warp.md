# Frame Warp

[Documentation](../index.md) · [Host feature support](../PARITY-MATRIX.md) · [Video and latency](../video-and-latency.md)

Status: version 1. Nightjar implements the client side, and Rubylight the host side. Any host can implement it; a client checks the advertisement, never the host name.

A camera turn reaches the screen a network round trip plus the game's own delay after the input. Frame Warp lets the client show it on its next refresh: it shifts each picture by the turn that its own mouse or right-stick input has not reached that picture with yet, and leaves what stays put on screen (crosshair, health, minimap, the weapon) where it is. The host does the learning: per game, how far the picture moves per mouse count and per second of stick, how long the game takes to show an input, and where the HUD is. It tags each frame with how much input the frame reflects. The client only adds up what it sent.

The shift is a translation. It is right for a camera turn near the screen's centre, approximate near its edges at wide fields of view, and does not predict movement such as strafing.

## Overview

1. The host advertises the extension in its serverinfo reply and in the RTSP DESCRIBE answer of each stream.
2. The client sends a **request** (`0x5543`) on the encrypted control stream once the stream runs.
3. The host learns a **model** of the running game and sends it (`0x5544`) when it locks, unlocks or changes.
4. Each video frame, as its number is assigned, gets a **tag** (`0x5545`): the input it reflects.
5. While the camera turns the host finds what stays put and sends a **HUD mask** (`0x5546`) when it changes.
6. The client computes a shift for every picture it draws and samples the picture at `position - shift`, except inside the mask.

## Advertising support

**serverinfo** (`/serverinfo`, HTTP and HTTPS):

```xml
<FrameWarpVersion>1</FrameWarpVersion>
```

The highest version of this document the host speaks, as a decimal integer. A missing field, `0` or anything that is not a decimal number from 1 to 65535 means no support.

**Per stream** (RTSP DESCRIBE answer, the line hosts use for host-to-client control message ids):

```text
a=x-rl-control:... 0x5543 0x5544 0x5545 0x5546
```

The ids may share the line with others. The client sends a request only when both are present. A host that turns the feature off for a stream leaves the ids out.

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

Shifts are in picture pixels (the stream's video size, `M.picture_width` by `M.picture_height`) and move the content: the client samples the picture at `position - shift`. Mouse y grows downward as in `MouseMove`; the stick's y is the input stream's (up is positive), and the model's sign carries the direction. `curve` is the stick response below. The sums wrap at 2³²; take differences as signed 32-bit values.

The client should compute the shift as late as it can before each present, and draw the same picture again with a new shift on refreshes between pictures. When nothing has been sent for 600 ms, both sums are equal to the host's (no input is in flight), so a lost count lasts until the next pause; Nightjar uses this to rebase. When the model unlocks or the client stops, the shift fades to zero over about 150 ms rather than jumping.

**Stick response** (both sides integrate the same function): with `vx = x / 32767` and `vy = y / 32767` clamped to -1..1, `r = sqrt(vx² + vy²)`; zero when `r` is at most the dead zone, otherwise `s = min((r - deadzone) / (1 - deadzone), 1)` and the result is `(vx / r, vy / r) * s^exponent`.

## Messages

All messages travel on the encrypted control stream (`SS_ENC_CONTROL_V2`). All integers are little-endian; floats are IEEE 754 binary32. A receiver drops a message that breaks any rule below. Every message may grow in a later version: a receiver reads the fields it knows and ignores trailing bytes.

### `0x5543` request, client to host (8 bytes, reliable)

| Offset | Size | Field | Rule |
| --- | --- | --- | --- |
| 0 | u8 | version | 1 |
| 1 | u8 | enabled | 1 = learn, tag and mask; 0 = stop all three |
| 2 | u8 | controller | Input stream controller number (0 to 15) whose right stick turns the camera; 0xFF for none |
| 3 | u8 | reserved | 0 |
| 4 | u16 | mask max width | Widest HUD mask the client takes, at most 2048; 0 asks for none |
| 6 | u16 | reserved | 0 |

An enable request that starts Frame Warp (the first, or the first after a disable) restarts the counts on both sides: the host counts the input it receives from then on, the client the input it sends from then on. Input and control messages travel on different channels, so the counts can still differ by what was in flight; the idle rule above settles that. A repeated enable request keeps the counts. Every enable request makes the host send its current model and mask again. A host stops when the stream ends.

### `0x5544` model, host to client (48 bytes, reliable)

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

The host sends a model when it locks or unlocks, and when a locked gain changes by more than a few percent. Neither part locked means: draw unshifted.

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

## Client behavior (Nightjar)

- **Frame Warp** is a Video setting, off by default. Nightjar sends a request only when the host advertises both the serverinfo field and the ids for the stream, and not for a remote monitor stream.
- It counts input at the point it goes on the wire, so what it adds up is exactly what the host received.
- Tags are parsed on the control thread; models and masks are checked there and applied atomically between pictures. Video decoding is untouched: the shift happens where the picture is drawn.
- The overlay shows the current shift, whether the model is locked, and how many pictures were shifted, extrapolated or untagged.

## Host requirements

- Count mouse motion and controller packets as they are received, before injection, in the same order the client sent them.
- Send nothing before an enable request, and stop when a disable request or the end of the stream arrives.
- Never send a mask wider than the request's mask max width, or a model whose values break the bounds above.
