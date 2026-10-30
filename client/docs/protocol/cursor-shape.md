# Host cursor shapes

[Documentation](../index.md) · [Host feature support](../PARITY-MATRIX.md) · [Controls](../controls.md#touch-modes)

Status: version 1. Nightjar implements the client side, and Rubylight 3.0 the host side. Any host can implement it; a client checks the advertisement, never the host name.

In Local cursor touch mode Nightjar draws the pointer on the phone, so it moves with no round trip to the PC. Without help from the host that pointer is always an arrow, and the PC's own cursor can still show in the video a frame or more behind it. This extension lets a host send its cursor image (arrow, I-beam, resize arrows, hand, a game's custom cursor) and whether it is visible, and leave the cursor out of the video while the client draws it.

Any host can implement it. Nothing here depends on a particular host; a client checks the fields and messages below, never the host's name.

## Overview

1. The host advertises the extension in its serverinfo reply and in the RTSP DESCRIBE answer of each stream.
2. When the user is in a mode that draws the pointer locally, the client sends a **request** (`0x5540`) on the encrypted control stream.
3. The host answers with **shape messages** (`0x5541`) whenever the cursor's image or visibility changes, and stops drawing the cursor into the captured video.
4. When the client leaves that mode it sends the request again with `enabled = 0`; the host goes back to capturing the cursor in the video and stops sending shapes.

The client keeps sending absolute mouse positions as it does today; this extension only changes how the pointer looks.

## Advertising support

**serverinfo** (`/serverinfo`, HTTP and HTTPS):

```xml
<CursorShapeVersion>1</CursorShapeVersion>
```

The highest version of this document the host speaks, as a decimal integer. A host that sends any version also speaks version 1. A missing field, `0` or anything that is not a decimal number from 1 to 65535 means no support. Nightjar shows **Cursor shapes** among the host's features when the field is present and keeps it with the PC's other stored capabilities.

**Per stream** (RTSP DESCRIBE answer, the same attribute hosts already use for client-to-host control messages):

```text
a=x-rl-control:0x5540
```

`0x5540` may share the line with other ids. The serverinfo field says the host build supports the extension; the DESCRIBE entry says this stream can use it (the host may leave it out, for example when it cannot hide the cursor from capture on the current display). The client sends a request only when both are present.

## Messages

Both messages travel on the encrypted control stream (`SS_ENC_CONTROL_V2`) as reliable ENet packets, so they arrive once and in order. All multi-byte fields are little-endian. A receiver drops a message that breaks any rule below, logs why, and draws its default pointer.

### `0x5540` request, client to host (4 bytes)

| Offset | Size | Field | Rule |
| --- | --- | --- | --- |
| 0 | u8 | version | 1 |
| 1 | u8 | enabled | 1 = send shapes and leave the cursor out of the video; 0 = stop |
| 2 | u16 | max edge | Largest width or height the client accepts, 1–96. Nightjar sends 96. |

Every enable request starts a fresh session: both sides empty their shape cache, and the host sends the current shape (or `HIDE`) right away. Sending enable again therefore resynchronizes a client that lost track. A disable request ends the session; the host stops sending shapes and draws the cursor into the video again. The host also ends the session when the stream ends.

### `0x5541` shape, host to client (8 bytes, or 20 + pixels for `IMAGE`)

Header:

| Offset | Size | Field | Rule |
| --- | --- | --- | --- |
| 0 | u8 | version | 1 |
| 1 | u8 | kind | 0 `HIDE`, 1 `DEFAULT`, 2 `SHOW`, 3 `IMAGE` |
| 2 | u16 | reserved | 0 |
| 4 | u32 | id | Cache id. 0 for `HIDE` and `DEFAULT`; non-zero for `SHOW` and `IMAGE`. |

Kinds:

- `HIDE` (8 bytes): the host cursor is hidden, for example while a game captures the mouse. The client draws no pointer until the next `DEFAULT`, `SHOW` or `IMAGE`.
- `DEFAULT` (8 bytes): show the client's own default pointer. A host sends it for a cursor it cannot convert.
- `SHOW` (8 bytes): show the image cached under `id`.
- `IMAGE` (20 bytes + pixels): define `id` with this image and show it.

`IMAGE` continues after the header:

| Offset | Size | Field | Rule |
| --- | --- | --- | --- |
| 8 | u16 | width | 1 to max edge (at most 96) |
| 10 | u16 | height | 1 to max edge (at most 96) |
| 12 | u16 | hotspot x | Less than width |
| 14 | u16 | hotspot y | Less than height |
| 16 | u8 | format | 1 = RGBA8888 |
| 17 | u8 | reserved | 0 |
| 18 | u16 | scale | The host display's scale in percent, 100–400 (100 = 96 DPI) |
| 20 | width × height × 4 | pixels | Rows top to bottom, no padding; per pixel R, G, B, A in sRGB with straight (not premultiplied) alpha |

The message length must be exactly 20 + width × height × 4 bytes, so the largest message is 36,884 bytes. Hosts convert monochrome and XOR cursors (such as the classic Windows I-beam) to RGBA, for example black with a white outline, or send `DEFAULT`. Animated cursors send one `IMAGE` or `SHOW` per frame change and should reuse ids for repeating frames.

**Size on screen.** One host pixel at 100% scale is drawn as one density-independent pixel (dp) on the client, so the same cursor looks the same size whether the host display runs at 100% or 200%. A host should send the cursor at its display's own scale and set `scale` to match.

## Caching

Defining an id with `IMAGE` and showing it with `SHOW` both make it the most recently used. The client keeps at least the 32 most recently used ids; the host tracks the same order over the messages it sent in this session and sends `IMAGE` again for any id that has dropped out of the last 32. An id that is redefined replaces its old image. A `SHOW` for an id the client does not have (for example after it rejected that image) shows the default pointer and is logged.

Ids are opaque to the client. A host may derive them from a hash of the image or from the operating system's cursor handle, as long as one id never names two different images within a session.

## Client behavior (Nightjar)

- Requests shapes only in Local cursor touch mode, only when the host advertises both the serverinfo field and `0x5540` for the stream, and sends `enabled = 0` when the user switches to another touch mode.
- Accepts `0x5541` only between its own enable and disable requests; anything else is dropped in the native layer.
- Checks size bounds on the control stream's receive thread and does everything else on a background thread: full validation, conversion, scaling and caching by id. Video, audio and input paths are not touched.
- On a message it cannot use, logs the reason, draws its own arrow and tells the user once per session that the PC's cursor shape could not be drawn.
- Without host support, nothing changes: the pointer stays Nightjar's arrow and the PC's cursor stays in the video.

## Host requirements

- Leave the cursor out of the captured video for the whole session (between enable and disable), and put it back afterwards.
- Send the current state right after an enable request, then only on changes. Coalesce rapid changes; a cursor rarely needs more than 60 messages per second.
- Never send `0x5541` to a client that has not enabled the session, and never send an image larger than the client's max edge.
