# Racing the beam with PyroWave: how Rubylight and Nightjar draw a frame before it has arrived

*Ramazan Kara, 2026-10-26. Experimental feature; every number here is our own measurement on one PC.*

PyroWave streams are big. At 400 Mbps a 1080p60 frame is about 830 KB, and on a 1 GbE link the
last byte of it lands several milliseconds after the first. A conventional client waits for that last
byte, decodes, and shows the whole picture. Beam racing removes the wait for most of the picture:
the host sends each frame top to bottom, the client decodes the top while the bottom is still on the
wire, and it presents each finished slice just before the display's scan-out reaches those rows.

This post covers how that works in Rubylight (the Windows host) and Nightjar desktop (the client),
from the wavelet maths that decides what "top to bottom" means, through the packetizer and the RTSP
negotiation, to reading the display's scan line and scheduling presents against it. It ends with what
we measured, which is less than we hoped: the race works, but on our loopback setup it missed the gate
we set for turning it on by default. It ships as an opt-in.

The idea came from [Vibepollo issue 607](https://github.com/Nonary/Vibepollo/issues/607). PyroWave
itself is [Themaister's codec](https://github.com/Themaister/pyrowave).

## Why the wire time matters for PyroWave

PyroWave is an intra-only wavelet codec built for very high bitrates on fast local networks. Every
frame stands alone, which makes loss cheap (a missing block blurs, it never smears into later frames)
and encode and decode fast (the GPU inverse wavelet plus dequantisation takes 0.06 ms at 1080p on our
RX 7900 XT). The price is size: frames are large and all about the same size, so a frame's time on the
wire is a fixed tax on every picture.

The 3.0 plan put a number on it: a 1 MB frame takes about 3 ms at 2.5 Gbps and about 8 ms on 1 GbE.
With H.264 or HEVC at 50 Mbps that time is a fraction of a millisecond and nobody cares. With PyroWave
it can be the largest single term in the picture's age, larger than capture, encode and decode
together.

You cannot make the bytes arrive sooner without a faster link. What you can do is stop waiting for
the last one before you show the first row.

## What a PyroWave frame looks like

A frame is an 8-byte sequence header followed by block records. Each record is one 32×32 block of
wavelet coefficients, entropy coded on its own (16 sub-blocks of 8×8 with ballot, bit-plane and sign
coding). A record's header holds its size and its block index, and a record decodes from its own bytes
alone, so the decoder accepts records in any order.

The block index space runs coarse to fine: wavelet level 4 first (the 12 coarse bands, the picture at
1/32 scale), then levels 3, 2, 1 and 0. Within a level it goes per component (Y, Cb, Cr), per band (HL,
LH, HH), and inside a band the blocks run row by row. At 1080p 4:2:0, level 0 luma is 960×544
coefficients per band, 30×17 blocks; level 4 is 2×2 blocks per band, 48 coarse records in all.

That order is natural for an encoder and useless for beam racing. Every band covers the whole
picture, and the inverse wavelet needs all levels of a region before any pixel of it exists. The finest
luma bands come last, so in index order the top row of the picture depends on records near the end of
the frame. Nothing can be reconstructed early.

## The host: from capture to paced packets

Rubylight's PyroWave path keeps the picture on the GPU until it is a bitstream:

1. Desktop Duplication or WGC captures into a D3D11 texture.
2. D3D12 compute converts it into three planar textures (Y, Cb, Cr; 8-bit, or 10-bit for HDR).
3. A shared fence orders the conversion against Vulkan, where the PyroWave encoder reads the same
   memory and writes the bitstream, which comes back to the CPU.
4. `record_frame` (in `rust/core/src/pyrowave.rs`) frames the bitstream for the network: the header,
   then the coarse records, then the fine ones, packed first-fit into packets so that no ordinary
   record straddles two. The coarse records are FEC block 0, the critical block every row needs;
   up to three more blocks of consecutive packets carry the detail.
5. The paced sender spreads the frame over `bytes × 8 / pace`, where the pace comes from the client's
   hints: `pyrowaveLinkMbps` (its NIC speed) and `pyrowavePaceMbps` (the rate its own calibration found
   loss-free). We prefer what the client reports to probing from the host.

Step 4 is where the row order lives.

### Row order

For beam racing the host has to send, before band *k* of the picture, every record that band *k*
depends on. A band here is 64 picture rows, one level-0 block row. The dependency is worked out once
from the inverse wavelet:

- A level-*L* block row *r* covers picture band *r × 2^L* onwards.
- Synthesis is four lifting steps, each reaching one coefficient up or down, and the steps are
  separable. Carried through the levels, the reach is 2, 3, 3.5, 3.75 and 3.9 coefficient rows for levels
  0 to 4, always under one block row.
- So band *k* needs level *L*, block row *r* when *r × 2^L − 1 ≤ k*: one band of lookahead at every level.

In code, that is the whole step table:

```rust
pub fn row_steps(width: u32, height: u32, chroma444: bool) -> Vec<u16> {
    let bands = (height.div_ceil(32).max(4) * 32).div_ceil(64);
    blocks(width, height, chroma444)
        .iter()
        .map(|b| {
            if b.level == 4 {
                0
            } else {
                ((b.row << b.level).saturating_sub(1)).min(bands - 1) as u16
            }
        })
        .collect()
}
```

With the row order on, `record_frame_with` keeps the coarse class exactly as before (so FEC block 0 and
its parity are byte-identical), then sorts the fine records by `(step, index)` and packs them first-fit
again. Records too big for one packet act as barriers: each goes out once every ordinary record ranked
before it has, and the records ranked after it may fill the packet it leaves open. A record that
first-fit skips leads the next packet, so no record arrives more than about one packet late. The same
records and the same bytes go out; only the order and the padding between packets change. Pacing,
partial recovery and the FEC plan are untouched, and the detail FEC blocks become horizontal slabs of
the picture instead of bands of levels.

Row order has a cost in padding, and it depends on the packet size. At 1360-byte packets a row-ordered
frame is about 3% larger than index order; at 1024 bytes, which Rubylight uses for a remote-class link,
it is about 20% larger. When the framed frame no longer fits the frame's wire budget, the host sends that
frame in index order and raises a warning event ("PyroWave row order was dropped for frames that do not
fit the wire budget ..."). Failures are loud: the client sees the order change and says so too.

### Negotiation

The client asks with a new RTSP key in the ANNOUNCE SDP, next to the pacing hints:

```
a=x-ss-video[0].pyrowaveRowOrder:1
```

It is a separate key rather than a bit in `pyrowaveFeatures`, so it cannot collide with how other
hosts parse that field, and `PROTOCOL_VERSION` stays where it is. Hosts that do not know the key ignore
it; clients that do not send it, including Nonary's Moonlight and older Nightjar builds, get the old
order byte for byte (a regression test pins it). The host does not confirm the order. The client
checks the stream itself.

## The client: Nightjar desktop

Beam racing in Nightjar is the last of four pieces, and the first three are useful on their own.

### 1. Early decode

The receive path hands each packet's records to the decoder thread as the packet completes, instead of
building the whole frame first. On its own this only moves the entropy decode and the plane clear into
the frame's wire time; with index order nothing can be shown early. It is the plumbing the rest stands on.

### 2. GPU decode with no copies

Nightjar loads the PyroWave SDK at run time and lets it create a Vulkan device on the same adapter as
the D3D11 presenter. The three planes are D3D11 textures created shared and imported into Vulkan, so
D3D11 samples the memory the SDK wrote. Two shared `ID3D11Fence`s, imported as timeline semaphores,
carry the ordering: the decode fence (D3D11 waits on it before sampling) and the reuse fence (the
presenter signals it after the last draw that read a plane set; the next decode into that set waits for
it). The handoff costs about 0.035 ms at the median at 1080p, 1440p and 4K.

There is no silent fallback. If the GPU decoder fails its probe, PyroWave is not offered and the
settings screen says why.

Against the CPU decoder, `receive_to_present` at 1080p and 133 Mbps went from 20.5 to 8.8 ms at the
median.

### 3. Present on decode

The next wait was the pacer. Presenting at the host's frame times held each frame about 7 ms to absorb
the spread of the host's capture-to-encode age. On a display with tearing and VRR, Nightjar now
presents the moment a frame is decoded. At 1080p and 300 Mbps on our 240 Hz VRR display, that took
`receive_to_present` from 10.36 to 3.21 ms at the median, and the p99 present-interval deviation from
9.24 to 1.96 ms. Showing frames at the PC's frame times is still a setting.

### 4. Slices

With the row order coming in, a `RowTracker` follows the records as packets complete. It cannot just
watch the latest record's step, because the host's gap fill puts small later records into packets early:
at 1080p and 399 Mbps we saw records up to 460 packets ahead of their turn. Instead it follows the lowest
step among the records each packet completes, a cursor that never goes back. A band is complete once the
cursor has passed its step in an earlier packet.

The picture is cut into four slices (three at 4K). When a slice's bands are complete, the decoder runs on
the frame's prefix: the blocks that have not arrived are zero, which the SDK supports. Because of the
one-band lookahead, the rows of that slice come out exact. An ignored GPU test checks this on real
row-ordered frames: every slice's rows are byte-identical to the final decode's, on 24 frames and 72 slice
decodes. The final decode of the complete frame always runs, so what ends up on screen is the whole
frame, never a prefix.

A frame that breaks the order (a host that does not send it, or loss that reorders more than one packet)
is not sliced from the first violation on and decodes whole. One late record is tolerated, but if a slice
was already decoded without it, the frame drops its slices and counts as unordered.

On a 60 s loopback stream at 1080p60, 399 Mbps and pace 900, every frame was row-ordered and sliced four
ways. The first slice was ready 1.65 ms after the frame's first packet at the median, while the frame took
6.48 ms to arrive. A quarter of the picture is on the GPU at a quarter of the wire time.

## Racing the beam

Having slices early only helps if they reach the screen early, so the presenter needs to know where the
display's scan-out is.

### Reading the scan line

Nightjar opens the window's monitor with `D3DKMTOpenAdapterFromGdiDisplayName` and polls
`D3DKMTGetScanLine`. On the RX 7900 XT driving a 5120×1440 240 Hz VRR panel, the counter runs at 393.6
lines per ms from 0: 186 to 205 blanking lines (`InVerticalBlank`) and then the 1440 picture rows. A fixed
refresh wraps it continuously, and the whole scan, blanking included, takes S = 4.1 to 4.2 ms (the active
rows alone 3.65 ms). The first second of readings calibrates the rate, the blanking, the rows and S.

VRR is what makes racing practical. A VRR panel that is idle between presents keeps counting past the end
of its scan with `InVerticalBlank` set, and a tearing present restarts the scan. So the client does not
have to hit a moving target: it starts the scan itself. On our display the scan starts a median 0.91 ms
after the present call, and row *r* is shown *r × row time* after that.

The client checks this rather than assuming it. A present issued while the display idles must start a new
scan within 10 ms (the longest delay we saw plus 1 ms; tighter windows failed on a loaded GPU), the first
reading out of the blanking within 5% of the picture rows. Nine misses in 30 tested presents fail the
check. A VRR panel's own refresh at its minimum rate looks like a restart, so a scan starting within 1 ms
of the longest idle seen is neither counted nor missed. The polling runs on a helper thread, so the render
thread only hands over the reading from before the present.

If any condition fails, the overlay says which and the stream presents whole pictures: no tearing, no scan
line, no scan seen, scan rows that do not match the display (a scaled or rotated desktop), not fullscreen,
no VRR scan restart, no VRR, or no idle time at this frame rate (a 240 fps stream on a 240 Hz panel leaves
nothing to race into).

### Scheduling the presents

With the probe passing, the pacing mode becomes `Beam race`. Per frame, the planner (`race::plan`) picks a
scan start *t0* such that every slice *i* makes it before the beam reaches its first row:

```
e_i + wake + call + margin <= t0 + delta + R_i
```

Here *e_i* is when slice *i* will be ready relative to the first, *delta* the measured restart delay, and
*R_i* when the beam reaches that slice's first row. The slice times are predicted from the pace hint for the
first frames and then from the frames shown (weight 1/8). *wake* (slice ready to present call) and *call*
(the present call itself) are medians over the last 240 presents. The margin is the p99 minus p50 of wake
plus call, plus one 1 ms host send batch, or a value the user sets. If racing a frame gains less than 1 ms,
it is presented whole and the overlay says why.

Each slice present redraws the persistent video texture with a scissor on that slice's rows (the rows above
are still there) and presents with tearing. The final decode of a raced frame draws only the rows after the
last slice presented. A slice that is ready after the beam has passed its first row is late: it is counted,
the rest of the frame is held, and the whole picture goes out on the final decode.

Two details took the longest to get right:

- **Plane reuse.** Decodes of the next slice write into the same plane set a draw may still be reading.
  That is safe only because the rows a later prefix decode rewrites hold the same bytes they already had
  (the exactness test again) and the rows it changes are outside the earlier draw's scissor. The presenter
  pins each slice's plane set, and the next decode into it waits on the draws issued so far.
- **Queue family ownership.** A plane set that holds a slice is acquired from `QUEUE_FAMILY_EXTERNAL`, which
  makes the SDK record a GENERAL to GENERAL transition. Acquiring from `QUEUE_FAMILY_IGNORED` records
  UNDEFINED to GENERAL, which allows the driver to discard the rows the presenter is still showing.

## What we measured

The gate: loopback to an isolated Rubylight host on the same PC (RX 7900 XT), Nightjar desktop in
borderless fullscreen on the 240 Hz VRR display, 1080p60 PyroWave 4:2:0 at 399 Mbps, release builds,
60 s legs in ABBA order, two runs a side, the first 300 frames of each run dropped. The A side is the same
build presenting whole pictures (`NIGHTJAR_BEAM_RACE_PRESENT=0`). The metric is the mid-row age: when the
beam reaches the picture's middle row minus when the slice holding it was ready. To become the default, the
median and the p95 had to gain at least min(W/2, S/2), where W is the frame's time on the wire at the pace
and S the scan, with `receive_to_present` p95 no worse and under 1% late slices.

| Leg | Required gain | Mid-row age p50, whole → raced | p95, whole → raced | `receive_to_present` p95 | Late slices |
|---|---|---|---|---|---|
| Pace 900 (W = 7.4 ms) | 2.08 ms | 5.32 → 3.93 ms (1.40) | 5.98 → 5.33 ms (0.65) | 8.64 → 8.61 ms | 0.061% |
| Pace 2100 (W = 3.17 ms) | 1.58 ms | 4.18 → 3.47 ms (0.71) | 4.79 → 4.19 ms (0.60) | 4.59 → 5.19 ms | 0.000% |
| Pace 900, 0.1% loss | 2.08 ms | 5.30 → 3.91 ms (1.39) | 5.88 → 5.45 ms (0.44) | 8.33 → 8.53 ms | 0.015% |
| Rerun, Oct 26: pace 900 | 2.08 ms | 5.49 → 4.32 ms (1.17) | 6.20 → 5.66 ms (0.54) | 8.34 → 8.67 ms | 0.080% |
| Rerun, Oct 26: pace 2100 | 1.58 ms | 4.72 → 3.77 ms (0.95) | 5.36 → 4.72 ms (0.64) | 4.68 → 5.66 ms | 0.023% |

The first three rows are the October 24 gate; the last two rerun its first two legs on October 26 with the
3.0.0-rc.2 host and a newer client build, one ABBA set each. Every run exited cleanly, with no dropped
frames, no decoder errors and no hang. The race works: the middle of the picture is on screen 1.2 to 1.4 ms
sooner at the median at pace 900. It missed the gate on every leg, and at pace 2100 `receive_to_present`
got 0.6 to 1.0 ms worse at p95. So it is not on by default.

Why the gain is smaller than the model:

- **The race trades the bottom for the top.** The middle row gains, but the last rows reach the screen later
  than a whole-picture present puts them. A raced frame averages 2.4 presents, each with an extra draw.
- **The overlay costs about 0.2 ms per present** when it is on, and a raced frame has 2.4 of them.
- **There is little to win at high pace.** At pace 2100 the frame is on the wire for 3.2 ms, the scan takes
  4.2 ms, and the host sends in batches of about 1 ms, so slices arrive in steps rather than continuously.
- **Loopback is pessimistic.** The host's encoder and the client's decoder and presenter share one GPU, so a
  raced frame competes with the encode of the next one.

Things we tried that did not move the numbers: a raw `Present1` with `ALLOW_TEARING` instead of
`SDL_RenderPresent`, and skipping the flush. Drawing consecutive slices of one plane set in a single pass
helped a little and is in the numbers above.

## A second PC over Wi-Fi

On October 26 we ran the same stream from this PC to a laptop (Radeon 780M, Wi-Fi 6E at 2402 Mbps, 5 GHz)
to get off loopback. It did not race. The laptop's 120 Hz panel scans all the time and a present never
found it idle, so the probe said "no VRR" and presented whole pictures, which is the designed behaviour.
What the run did show is the row order over real Wi-Fi: in one ABBA set at 399 Mbps every frame stayed in
row order, no slice was late, the first slice was ready 2.4 to 2.6 ms after the frame's first packet (1.65 ms
on loopback), and turning the row order on cost nothing we could separate from the 2 ms spread between
runs. The numbers are in [PERFORMANCE.md](../rust/PERFORMANCE.md#october-26-beam-raced-pyrowave-over-wi-fi-6e-to-the-780m-laptop-b10-quick-pass).

## Status and how to try it

The case the idea was built for, a separate PC on wired 1 GbE or 2.5 GbE, has not been measured yet, and
neither has a margin sweep or a raced run on any client but this PC. Until they are, beam racing is
Experimental and off by default.

- **Host:** Rubylight 3.0 sends the row order when a client asks for it. Nothing to configure.
- **Client:** Nightjar desktop on Windows, the `beam_race` setting (or `NIGHTJAR_BEAM_RACE=1`). It needs
  PyroWave with GPU decode, exclusive or borderless fullscreen, a display with tearing and VRR, and a wired
  link hint. The stats overlay's "Beam race" and "Raced presenter" lines say whether it is racing and, if
  not, why.
- **Off means off.** With the setting off the client asks for no row order, starts no probe and decodes
  whole frames; tests pin all of that.

Android is out of scope. The client asks for the row order on Wi-Fi only when told to (the Wi-Fi run
above forced it with `NIGHTJAR_PYROWAVE_LINK_MBPS`), and a raced present over Wi-Fi has not been measured. On Linux, Wayland compositors cannot race the beam either; it would need direct scan-out through a
KMS lease or similar.

The full records are in Rubylight's [PERFORMANCE.md](../rust/PERFORMANCE.md#october-24-beam-raced-pyrowave-gate-b10-step-4-opt-in-missed-its-default-on-gate)
and the [3.0 plan](rubylight-3.0-plan.md) (B10), and in Nightjar's `docs/beam-racing.md`,
`docs/pyrowave-decode.md` and `docs/early-decode.md`.
