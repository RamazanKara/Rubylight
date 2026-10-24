# Beam racing (Rubylight 3.0 plan, B10 step 4)

Experimental and opt-in. It missed its default-on gate on loopback (the numbers and the decision are
in [pyrowave-decode.md](pyrowave-decode.md#beam-racing-measured-opt-in-helps-the-median-at-low-pace-not-p95-worse-receive_to_present-at-high-pace));
the wired NUC legs have not run. Turn it on with the `beam_race` setting or `NIGHTJAR_BEAM_RACE=1`.
`NIGHTJAR_BEAM_RACE=0` forces it off. The switches take `1`, `true`, `yes`, `on` and `0`, `false`, `no`, `off` in any case; any other value is reported on stderr and ignored, and so is a margin that is not a number. `NIGHTJAR_BEAM_RACE_SLICES` is 1 to 7 (a frame's presents are kept in 8 timing columns). `beam_race_margin_ms` (`NIGHTJAR_BEAM_RACE_MARGIN_MS`),
`NIGHTJAR_BEAM_RACE_SLICES` and `NIGHTJAR_BEAM_RACE_PRESENT=0` (probe and slices, whole presents) are
for measurement runs.

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
seen to finish) on the decoded picture. The raced presenter draws from them (below).

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
| scan rows differ from the display's | the counter's top is more than 3% off the blanking plus the monitor's height: a scaled desktop or a rotated display, whose rows are not the picture's |
| not fullscreen | the window is not a fullscreen window (the scan is not checked) |
| no VRR scan restart | the scan-restart check failed on a fullscreen window: a present issued while the display idled did not start a scan, so VRR is not active for this window |
| no VRR | the restart check failed on a display that scans all the time (under 30% of the readings in the blanking at a stream rate under 70% of the display's), or the display never idled for 300 presents at a stream rate under 70% of its own |
| no idle at this frame rate | the display never idled for 300 presents at a stream rate of 70% of its own or more: a 240 fps stream on a 240 Hz panel leaves nothing to test |

The scan-restart check: a present issued while the display idles after a scan has to start a new
scan, the first reading out of the blanking within 5% of the picture rows. "Idles" is a `vblank`
reading at or past `idle_from`, which calibration derives: past the last row by the blanking or 1 ms
of lines, whichever is more, and past every line a scan that ran straight into the next one (within
1.5 scans) counted in a trailing blanking. A fixed refresh wraps to line 0 and never reaches it;
a present in the blanking of a scan under way proves nothing. The check looks for 10 ms after the
present call returned, fails at 9 misses in 30 tested presents, and runs its polling on a helper
thread (`beam-restart`), so the render thread only hands over the reading from before the present.
A VRR panel also refreshes by itself at its minimum rate, and that scan would look like a restart:
a new scan that starts within 1 ms of the longest idle seen so far (calibration plus the presents
since) after the last one began is not counted, tested or missed (unit tested for 60 fps against a
48 Hz floor; on this PC the display never idled long enough to see a floor). When the window moves
to another monitor the probe measures it again, drops the old display's calibration if it finishes
late (a generation tag), and says "no scanline" if the new monitor's scan line cannot be opened.

The window comes from measurement. The flip waits for the GPU work queued ahead of it, so the scan
begins anywhere between 0.2 ms and the time that work takes, not at a fixed delay. With the host
encoding on the same GPU, 60 presents (two earlier traces) spread evenly over 0.2-8.8 ms after the
present call, median 5 ms; a lighter run had median 0.95 ms and 92% under 2 ms. The first design's
0.5 ms window and the second's 3 ms failed these setups (1.7-2.3 ms medians, 9 of 10-13 tested
presents missed in the four runs on a quiet GPU); 10 ms is the longest seen plus 1 ms. A fixed
refresh gives no tested presents and ends as "no VRR" after 300 presents (unit tested, not run on
hardware: this PC's display is VRR). The capability's `flip_inferred` is exactly that, an
inference from the scan restarting on our presents, not an answer from the OS.

All of it was seen with this driver's counter layout (the RX 7900 XT on this PC: blanking first,
then the rows, then a counter that keeps counting while the display idles). A driver that lays the
counter out differently (the blanking after the picture, a counter that wraps while idling) is not
covered by hardware runs; the unit tests cover a trailing blanking and a counter that does not
idle past the picture.

Measured (30 s loopback streams, 2560x1440@60 PyroWave, GPU decode, borderless fullscreen, `Immediate`):

| Setup | Overlay | PresentMon `PresentMode` |
|---|---|---|
| window excluded from the host's capture (the harness default), 4 runs, window 10 ms | on in 4 of 4: 4.14-4.19 ms, 1440 rows; restart delay median 0.9-4.6 ms, longest 1.5-11.2 ms | Hardware: Independent Flip 97-98% |
| the same, 3 ms window, 4 runs on a quiet GPU | off (no VRR scan restart) in 4 of 4: 9 of 13-14 tested presents began later than 3 ms | Hardware: Independent Flip 97-98% |
| window visible to the host's capture (`-CaptureVisible`), 3 ms window | off (no VRR scan restart): 9 of 16 | Hardware: Independent Flip 97% |
| windowed (`window_mode` Windowed) | off (not fullscreen) | Hardware: Independent Flip 97% (multi-plane overlay); the scan was 13% blanking, a fixed refresh |
| V-Sync | off (no tearing) | Hardware Composed: Independent Flip |

The reason says what was measured. The probe has no signal for composition: "no VRR scan restart"
means a present did not restart the scan, which is also what a composed present would do, but
PresentMon reported independent flip for every one of these runs, so read it for what the OS did.
`NIGHTJAR_EXCLUDE_FROM_CAPTURE=1` does not force composition on this PC (the window visible to
capture still shows Independent Flip). The summary line `beam race: restart check over N presents
...` in the client log gives the counts and delays (measured from just before the present call,
so a slow call counts against the window's 10 ms in the longest figure but not in the check).

`scripts/pyrowave-loopback.ps1 -PresentMon <PresentMon 2.x console exe>` (or
`NIGHTJAR_PRESENTMON`) captures the client's presents into `<leg>.presentmon.csv`, and
`timing_summary.py` prints its `PresentMode` counts and the leg's `race_reason` counts. Settings
go in with `-ExtraA @{ beam_race = $true; window_mode = 'Borderless'; vrr = $true; present_at_host_times = $false }`.

## The raced presenter

With the capability probe on and a restart delay measured, the pacing mode goes from `Immediate` to
`Beam race` (the overlay's Pacing line; `PacingMode::BeamRace`, `stream/race.rs` and `stream/mod.rs`).
With `beam_race` off, or the probe off, the mode never changes and nothing here runs.

A frame still being received is presented in pieces. A tearing present on an idle variable refresh
display restarts the scan, and the first picture row is on the screen `delta` (the probe's median
restart delay) after the call, row `r` `r * row_ns` after that. So the first slice is presented at the
scan start `t0`, and each later slice is presented as it is ready, before the beam reaches its first
row; each present draws the persistent video texture again (a scissor on the slice's rows, the rows
above are still there) and presents it.

- `t0` is chosen per frame (`race::plan`). Slice `i` ready at `e_i` (relative to the first) must satisfy
  `e_i + wake + call + margin <= t0 + delta + R_i`, where `R_i` is when the beam reaches its row. The
  slice ready times come from the pace hint for the first frames and from the frames shown after
  (`Predictor`, weight 1/8). A frame is raced only when `first_ready + span + wake - t0` is at least
  1 ms, otherwise it is presented whole on its final decode and the overlay says "pace leaves under
  1 ms to gain".
- `wake` is the median time from a slice being ready (or the scan being due, when the plan holds it on
  purpose) to its present call, `call` the median call, both over the last 240 presents, folded every
  5 s (`Latency`). The whole pictures' present calls teach `call` before the first slice goes out.
  The margin is the p99 minus p50 of wake plus call, plus one 1 ms host batch, or the "Beam race
  margin (ms)" setting (`beam_race_margin_ms`, `NIGHTJAR_BEAM_RACE_MARGIN_MS`) when it is not 0.
- A slice that is ready after the beam passed its first row is late: it is counted (once per slice),
  the rest of the frame is held, and the whole picture is presented on the final decode. A frame
  whose first slice was not the picture's first, or that was not row ordered, is presented whole with
  the reason on the overlay. The final decode of a raced frame draws only the rows after the last
  slice presented.
- The decode of a plane set must not change rows a draw still reads. The presenter pins the set of
  each slice it is handed (`gpu_planes::Pin`), the draw signals the set's reuse fence, and the next
  decode into the set (the final one, or the next frame's) waits for the draws issued so far
  (`Planes::wait_for`). The decode of the next slice into the same set does not wait for a draw not
  issued yet, and does not need to: it rewrites the earlier slice's rows with the bytes they have
  already, because those rows depend only on records the earlier prefix had (the exactness test
  above), and the rows it changes are outside the earlier draw's scissor. A set that holds a
  slice is acquired from `QUEUE_FAMILY_EXTERNAL` (`decode_again`), which makes the SDK record
  GENERAL to GENERAL; acquiring from `QUEUE_FAMILY_IGNORED` records UNDEFINED to GENERAL and would let
  the driver discard the held rows. With the race off none
  of this runs: the decode waits for nothing it did not before, and a whole picture draws with the
  default rasterizer state, no scissor.
- A picture older than a frame whose slices are on the screen is dropped (counted as replaced), so
  the image never steps back a frame. Only the frame the racer follows is closed by its picture.
  When the display stops allowing racing the racer lets go of the slices it holds.
- The stream rows are the display's rows scaled to the window (`Scan::for_picture`); a letterboxed
  picture adds its top offset to the delay.

The overlay's "Raced presenter" line reads "N raced, M whole, K late slices in J frames", with
"; last whole: <reason>" and "N never finished" when they apply, or "off, pacing is X" and "off until
the display check passes". The client log prints the same at the end, with the count of frames per
reason and the wake, call and margin it ended with.

Four more timing CSV columns say how each frame went to the screen (empty when the race is off or
the scan is not known): `scan_start_ns` (the first present's issue time plus delta), `present_ns` (the
issue time of every present of the frame, `|` separated, up to 8), `mid_row_age_ns` (when the beam
reaches the middle row minus when the slice that holds it was ready, for raced frames and whole ones
alike) and `late_slices`. `ready_us` is when the decoder finished the slice, which stands in for when
its last packet arrived (the two differ by about the slice's decode, 0.2 ms). Setting
`NIGHTJAR_BEAM_RACE_PRESENT=0` keeps everything but the mode switch: the probe runs and
`mid_row_age_ns` is recorded for whole pictures, the A side of a comparison.

Measured (60 s loopback, 1080p60 PyroWave 399 Mbps, pace 900, `NIGHTJAR_STREAM_LOCATION=local`,
borderless fullscreen on the 5120x1440 240 Hz VRR display, one run per side):

| | raced | whole (`NIGHTJAR_BEAM_RACE_PRESENT=0`) |
|---|---|---|
| frames raced | 2965 of 3470 (462 skipped for gain, mostly while the margin settled) | 0 |
| late slices | 5 in 5 frames of 13880 slices (0.04%) | |
| mid-row age p50 / p95 / p99 (ms) | 4.89 / 6.04 / 6.53 (raced frames alone 4.81 / 5.99) | 5.53 / 6.13 / 6.41 |
| receive_to_present p50 / p95 (ms) | 8.46 / 9.36 | 7.96 / 8.62 |
| presents per frame | 2.40 | 1.00 |
| PresentMon | Independent Flip 99.7% (21 composed overlay, 6 composed) | |
| present call p50 / p95 | 260 / 316 us (wake 0.97 ms, margin 2.3 ms) | 209 / 315 us |

The raced frame shows its middle row 0.64 ms (median) earlier, and its last rows later: the frame as a
whole reaches the screen 0.5 ms after a whole-picture present does (the extra slice draws and the
rows-only final), so the race trades the bottom for the top. At 1080p and pace 900 the slices of a
frame arrive over 6.5 ms and the scan takes 4.1 ms, which is why the gain is about 1 ms.
The `SDL_RenderPresent` call is the only present path. A raw `Present1(0, ALLOW_TEARING)` on SDL's
swap chain was tried in brief 4 and did not move the numbers (see `pyrowave-decode.md`).

Known limits. A texture re-created or resized between the slices and the final decode leaves the top
rows black for that frame. The first frame is presented whole (the stream's setup is not known before
it). The Unix build does not race (the plane pins and slice draws are Windows only; not built here).

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
- `stream::race::tests`: the scheduler on synthetic traces (gain test, margin, the scan start for
  each slice, the first frames from the pace), the scissor maths, the mid-row age for raced and
  whole frames, the racer states (late slice, late final, abandoned frame, whole when not row
  ordered), and what the latency window folds.
- `timing_csv`: the four presented columns.
- `stream::tests::the_beam_race_off_asks_for_nothing` and `stream::pipeline::tests`: with the setting
  off (or the link or decoder unable to race) no row order is requested, early decode stays as the user
  set it, no slices are asked for (so no probe is created and no slice reaches the presenter) and
  nothing wakes the presenter; `sdp` tests: no `pyrowaveRowOrder` line without the request.
