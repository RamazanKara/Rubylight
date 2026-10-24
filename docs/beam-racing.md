# Beam racing (Rubylight 3.0 plan, B10 step 4)

The Nightjar desktop client can decode a PyroWave picture in slices, top to bottom, while the
rest of the frame is still arriving, so the top of the picture is on the GPU before the bottom
has left the network. It needs three things: the host sending the row order, early decode
(chunks decoded while a frame arrives, see [early-decode.md](early-decode.md)), and the GPU
PyroWave decoder. It is the `beam_race` setting (off by default) or `NIGHTJAR_BEAM_RACE=1`
for a measurement run. With it off nothing changes: no request, no slices, the same decode.

## The row order

With `x-ss-video[0].pyrowaveRowOrder:1` in the RTSP request, a Rubylight host sends the coarse
wavelet level first and then the fine records by 64-row picture band (block row `r` of level
`L` goes with band `r * 2^L - 1`, one band of lookahead for the inverse wavelet's lifting
steps). Every record with a step up to `k` having arrived is exactly what band `k` needs.
The step table is shared by the host (`rust/core/src/pyrowave.rs`) and the client
(`protocol/core/src/pyrowave/rows.rs`, pinned to the host's values by test vectors).

The client asks only when the link is a known wired one (a link hint is set); on any other
link it says so on stderr and decodes whole. `NIGHTJAR_PYROWAVE_LINK_MBPS` and
`NIGHTJAR_PYROWAVE_PACE_MBPS` override the hints for loopback runs.

## Slices

A `SlicePlan` follows the records as the packets complete (`RowTracker`). The host fills the
room a packet has left with any later record that fits, so a packet can carry records of far
later steps (measured at 1080p, 399 Mbps: up to 460 packets ahead of their turn). What follows
the row order is the lowest step among the records each packet completes, a cursor that does
not go back. A band is complete once the cursor passed its step in an earlier packet. A record
is out of place when it arrives more than one packet after that. The picture is cut in 4 slices (3 at 4K). When a slice's
bands are complete the decoder is run on the prefix of the frame: the header was pushed with
the first packet, the blocks not yet arrived are zero, and the SDK needs no `is_ready` for
that. The result lands in the plane set the final decode will fill, and the decode is waited
for, because the SDK keeps one payload buffer. The final decode of the complete frame is the
last slice and always runs, so the picture on screen is the whole frame's, never a prefix.

Each slice is a `BandReady` (slice number, first and end row, decode fence value, when it was
seen to finish) on the decoded picture. The presenter does not use them yet.

A frame that arrives out of row order (a host that does not send it, or loss that reorders
more than one packet) has `row_ordered` false, is never sliced from the first violation on, and
decodes whole. The overlay and the session summary count those, and say so when no frame has
been in order. One late record is tolerated as ordered (the host's packing can cause it), but a
slice decoded before it came lacks it: such a frame drops its slices, decodes whole, and counts
as unordered, which starts the run of ordered frames over. A late record before the first slice
costs nothing and still counts as ordered. Slices are therefore only ever reported for frames
with no late record at all, and the final decode fixes the rest of the picture.

## Measured

Loopback, 60 s, 1080p60, 399 Mbps, pace 900 (frame time on the wire W = 6.5 ms at p50), beam race
on, 3589 frames: every frame decoded early, 4 slices on every frame, `row_ordered` on 100%,
`first_slice_ready - receive` p50 1.65 ms (p95 2.14), `last_slice_ready - receive` p50 7.20 ms
(p95 7.92). The first slice is ready at 0.25 W. The host log shows `pyrowave_row_order=true`.
The ignored GPU test passed on 24 of these frames (72 slice decodes, each exact on its rows).

Two things a loopback run needs. The client treats 127.0.0.1 as a remote link (it is not a
private address), so the host sends 1024 byte packets, and a row-ordered frame in packets that
small does not fit its wire budget: the host sends it in index order, with a warning event
(`PyroWave row order was dropped ...`). `NIGHTJAR_STREAM_LOCATION=local` makes the stream local
(1360 byte packets, row-order framing about 3% larger than index order; at 1024 bytes it is
about 20% larger). And the host version must parse: `3.0.0-alpha` and `3.0.0-beta` versions did not (`-rc` always did).

## The timing CSV

Four columns after the early decode ones: `slices_decoded` (slice decodes for the frame, the
final one included), `first_slice_ready_ns` and `last_slice_ready_ns` (when the first slice
and the last decode finished), `row_ordered` (1 or 0). They are empty when the race is off or
the frame did not use early chunks. Compare `first_slice_ready_ns` with `receive_ns` (the
frame's first packet) against the frame's time on the wire.

## The capability probe

With the race on, the client reads the scan line of the window's monitor (`D3DKMTGetScanLine`,
opened from the monitor's GDI name, reopened when the window moves to another monitor) and says
on the overlay whether racing can work: "Beam race: on, scan 4.18 ms over 1440 rows" or "off
(reason)", with the same word in the timing CSV's `race_reason` column next to
`scan_line_at_present` and `in_vblank_at_present` (the beam when each present was issued). The
old slices line is now "Race slices". Nothing is read with `beam_race` off.

What the counter does (RX 7900 XT, 5120x1440 at 240 Hz, VRR): it counts lines at 393.6 per ms
from 0; the first 186-205 lines are the blanking (`InVerticalBlank`), then the 1440 picture rows.
A fixed refresh runs the counter around continuously: the whole scan, blanking included, is the
scan time S = 4.1-4.2 ms (the active rows alone take 3.65 ms). On a VRR display that idles
between presents the counter keeps counting past the end of the scan (up to 7000 lines seen at 60
fps) with `InVerticalBlank` set, and a present restarts it. The first second of readings gives the
rate, the blanking, the rows and S (calibration, `scanline.rs`).

The verdict, in the order of the reasons:

| Reason | When |
|---|---|
| no tearing | V-Sync pacing, or a swap chain without tearing |
| no scanline | `D3DKMTOpenAdapterFromGdiDisplayName` or `D3DKMTGetScanLine` fails |
| no scan seen | the calibration found no scan |
| composed | the window is not a fullscreen window, or the scan-restart check failed on a fullscreen one |
| no VRR | the restart check failed on a display that scans all the time (under 30% of the readings in the blanking at a stream rate under 70% of the display's), or the display never idled for 300 presents |

The scan-restart check: a present issued while the display idles in its blanking has to start a
scan, the first reading out of the blanking within 5% of the picture rows. It looks for 3 ms after
the present. Measured with independent flip on the VRR display, the scan begins about 1 ms after
the present call (median 0.95 ms, 89% under 1.5 ms, 92% under 2 ms, the rest up to 6 ms: the flip
waits for the GPU), so the first design's 0.5 ms window and 3 misses in 30 failed a good setup. It
now fails at 9 misses in 30 tested presents.

Measured (30 s loopback streams, 2560x1440@60 PyroWave, GPU decode, borderless fullscreen, `Immediate`):

| Setup | Overlay | PresentMon `PresentMode` |
|---|---|---|
| window excluded from the host's capture (the harness default), first run | on, 4.18 ms, 1440 rows | Hardware: Independent Flip 98% |
| the same setting, three later runs | off (composed): 9 of 10-12 tested presents did not start a scan in 3 ms | Hardware: Independent Flip |
| window visible to the host's capture (`-CaptureVisible`) | off (composed): 9 of 16 tested presents did not start a scan in 3 ms | Hardware: Independent Flip 97% |
| windowed (`window_mode` Windowed) | off (composed) | Hardware: Independent Flip 97% (multi-plane overlay); the scan was 13% blanking, a fixed refresh |
| V-Sync | off (no tearing) | Hardware Composed: Independent Flip |

`NIGHTJAR_EXCLUDE_FROM_CAPTURE=1` does not force composition here. The probe and PresentMon
disagree when the host captures the window: the OS calls it independent flip, but the scan does
not follow the presents within 3 ms (probably the GPU work queued ahead of the flip, with the host
capturing and encoding on the same GPU). Treat "composed" as "the display's scan does not start
with our presents" and read PresentMon for what the OS did.

The verdict is not stable on this PC: the same excluded borderless setting gave "on" in two runs and
"composed" in four later ones, with the scan itself measured the same each time (4.16-4.19 ms). In
the failing runs only 1 to 6 of the tested presents began a scan inside the window (median 1.7-2.2
ms, longest 3.1 ms), so the flips were late rather than absent. Other agents were using the GPU
during these runs, which is the likely cause, but a quiet-GPU rerun was not done. The summary line
`beam race: restart check over N presents ...` in the client log gives the counts and delays.
Brief 3 should treat "composed" as a reason to stay off, never as a reason to race, and the window
may need to grow if a quiet GPU still misses.

`scripts/pyrowave-loopback.ps1 -PresentMon <PresentMon 2.x console exe>` (or
`NIGHTJAR_PRESENTMON`) captures the client's presents into `<leg>.presentmon.csv`, and
`timing_summary.py` prints its `PresentMode` counts and the leg's `race_reason` counts. Settings
go in with `-ExtraA @{ beam_race = $true; window_mode = 'Borderless'; vrr = $true; present_at_host_times = $false }`.

## Tests

- `stream::pyrowave::tests`: the slice plan on 1080p row-ordered and index-ordered frames, and
  the shared step table against the old test-local one in the exact-prefix test.
- `timing_csv`: the four columns.
- Ignored GPU test `a_slice_decode_is_exact_on_its_rows_on_real_row_ordered_frames`: with
  `NIGHTJAR_PYROWAVE_DUMP` envelopes of a row-ordered stream in `NIGHTJAR_PYROWAVE_FRAMES`,
  every slice's rows are byte-identical to the final decode's.
- `stream::scanline::tests`: the calibration on synthetic fixed and variable refresh displays, the
  restart check, and each reason.
- `timing_csv`: the three scan columns.
- `scripts/test_timing_summary.py`: the PresentMode and verdict counts.
