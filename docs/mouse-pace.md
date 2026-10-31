# Mouse motion at the mouse's pace (Windows desktop)

A captured mouse (relative mode) sends its motion from SDL's raw input thread as each report
arrives (`protocol/desktop/src/stream/direct_motion.rs`), and streams on Wi-Fi put the adapter in
Windows' media streaming mode (`stream/wlan.rs`). Android already does both: it asks for unbuffered
input dispatch and holds a low-latency Wi-Fi lock. On Linux, SDL's X11 and Wayland backends push
mouse events on the main thread while it pumps events, so the filter runs there at the old pace.

## Why

The main loop takes SDL's events once per pass, and with V-Sync a pass waits in the present for
most of a refresh. On the 780M laptop (2880x1800, 120 Hz) a 1000 Hz mouse reached the PC about
120 times a second, phase-locked to the refresh, 5 to 12 ms apart in turn: a steady swipe moved the
PC's pointer in uneven steps. The client's network thread was not the cause; the loopback test
`unbatched_mouse_motion_reaches_the_host_at_the_mouse_pace` sends 1 kHz moves from another thread
and, on an idle machine, sees them leave about 1 ms apart (a busy one merges some, as it should;
the laptop measurement below is the check for the real path).

## How

An SDL event filter runs on the thread that pushes the event. For captured, relative, focused
motion it scales the move (mouse speed, fractions carried over), queues it on the session's input
queue (`Session::input_handle`) and drops the event. Clicks and wheel turns still go through the
main loop; while one waits there, later motion waits behind it, so motion never overtakes a click.
Touch and pen (with touch on) and the touchpad's pointer moves while touchpad contacts go to the PC
stay on the main loop, which already handles them.

The once-per-refresh pace used to merge a 4 or 8 kHz mouse's reports; now the input queue does:
relative motion leaves at most once per 0.5 ms after the last packet (`FAST_MOUSE_FLOOR`) even
with "Instant mouse and pen input", instead of one packet per report, which on Wi-Fi would take
airtime from the video. A 1 ms floor (Moonlight's batching interval) was tried first and measured
worse at 1 kHz: the floor counts from the send, which jitters, and Windows rounds the control
thread's wait up to a whole millisecond, so a report a little early waited a millisecond (959
packets/s, gap p90 1.6 ms against about 1100/s and 1.04 ms).

`NIGHTJAR_DIRECT_MOTION=0` keeps all motion on the main loop and `NIGHTJAR_WLAN_STREAMING=0` leaves
the Wi-Fi mode alone, for measurements only. The log ends each stream with
`mouse moves: N sent as they arrived, M through the main loop`.

## Measured

2026-10-31, laptop on Wi-Fi 6E (RZ616, AC power) streaming the desktop from the RX 7900 XT PC with
V-Sync on and the latency frame pacing. A script moved the mouse 2 px every 1 ms for 25 s (1000 Hz,
direction flipping every 500 ms); the PC's pointer was sampled at the PC and the laptop's uplink
captured with pktmon.

| arm | PC pointer updates/s | gap p90 | per-frame motion CV | laptop packets/s (gap p90) |
|---|---|---|---|---|
| before | 185 | 13.6 ms | 0.94 | 553 (4.8 ms) |
| Wi-Fi mode only | 126 | 16.9 ms | 1.01 | 504 (5.7 ms) |
| direct motion only | 90, 402 | stalls to 90 ms, 5.9 ms | –, 0.62 | 1097, 1106 (1.04 ms) |
| both | 682, 511, 576, 77 | 2.9, 4.0, 3.5 ms, stalls to 110 ms | 0.30, 0.49, 0.41, – | 959 to 1143 (1.04 to 1.6 ms) |
| both, 8 kHz mouse (1 px per 125 us) | 277 | 9.2 ms | 0.82 | 1188 (1.1 ms) |

Every arm delivered the full distance. With direct motion the laptop sends one packet per
millisecond, and in no run did its uplink have a gap over 20 ms; with an 8 kHz mouse the 1 ms floor
holds it to about 1200 packets/s. What the PC receives depends on the link: in 2 of 7 direct-motion
runs (one with the Wi-Fi mode, one without) the Wi-Fi delivered the input in clumps 50 to 110 ms
apart while the PC was idle, so a bad Wi-Fi moment still shows. With direct motion, runs with the
Wi-Fi mode reached 682, 511, 576 and 77 updates/s, runs without it 90 and 402; alone it changes
nothing while motion still leaves once per refresh.
