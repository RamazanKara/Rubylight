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

A `SlicePlan` follows the records as the packets complete (`RowTracker`): a band is complete
once a record of a later step has arrived in an earlier packet, because the host's packing
delays a record by at most one packet. The picture is cut in 4 slices (3 at 4K). When a slice's
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
been in order. A slice decoded before the first violation was seen is a prefix of the order
only as far as that frame went; the final decode fixes the picture, the flag fixes the count.

## The timing CSV

Four columns after the early decode ones: `slices_decoded` (slice decodes for the frame, the
final one included), `first_slice_ready_ns` and `last_slice_ready_ns` (when the first slice
and the last decode finished), `row_ordered` (1 or 0). They are empty when the race is off or
the frame did not use early chunks. Compare `first_slice_ready_ns` with `receive_ns` (the
frame's first packet) against the frame's time on the wire.

## Tests

- `stream::pyrowave::tests`: the slice plan on 1080p row-ordered and index-ordered frames, and
  the shared step table against the old test-local one in the exact-prefix test.
- `timing_csv`: the four columns.
- Ignored GPU test `a_slice_decode_is_exact_on_its_rows_on_real_row_ordered_frames`: with
  `NIGHTJAR_PYROWAVE_DUMP` envelopes of a row-ordered stream in `NIGHTJAR_PYROWAVE_FRAMES`,
  every slice's rows are byte-identical to the final decode's.
