# Mouse motion at the mouse's pace, input without waiting for the present (desktop)

A captured mouse (relative mode) sends its motion from SDL's raw input thread as each report
arrives (`protocol/desktop/src/stream/direct_motion.rs`), and streams on Wi-Fi put the adapter in
Windows' media streaming mode (`stream/wlan.rs`). Android already does both: it asks for unbuffered
input dispatch and holds a low-latency Wi-Fi lock. On Linux, SDL's X11 and Wayland backends push
mouse events on the main thread while it pumps events, so the filter runs there at the old pace.

Keys, clicks, wheel turns and controllers leave the same filter while the main loop waits in the
present ([below](#keys-clicks-and-controllers)): on Windows all of them, on Linux controllers
(keys and clicks come from the main thread there too).

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
queue (`Session::input_handle`) and drops the event. A click or wheel turn the main loop sends
holds later motion back while it waits there, so motion never overtakes a click.
Touch and pen (with touch on) and the touchpad's pointer moves while touchpad contacts go to the PC
stay on the main loop, which already handles them.

The once-per-refresh pace used to merge a 4 or 8 kHz mouse's reports; now the input queue does:
relative motion leaves at most once per 0.5 ms after the last packet (`FAST_MOUSE_FLOOR`) even
with "Instant mouse and pen input", instead of one packet per report, which on Wi-Fi would take
airtime from the video. A 1 ms floor (Moonlight's batching interval) was tried first and measured
worse at 1 kHz: the floor counts from the send, which jitters, and Windows rounds the control
thread's wait up to a whole millisecond, so a report a little early waited a millisecond (959
packets/s, gap p90 1.6 ms against about 1100/s and 1.04 ms).

`NIGHTJAR_DIRECT_MOTION=0` keeps all input on the main loop, `NIGHTJAR_DIRECT_INPUT=0` keeps keys,
clicks and controllers there (motion still leaves from the filter), and `NIGHTJAR_WLAN_STREAMING=0`
leaves the Wi-Fi mode alone, for measurements only. The log ends each stream with
`mouse moves: N sent as they arrived, M through the main loop` and
`keys, clicks and controller events: N sent during the present`.

## Keys, clicks and controllers

The main loop reads keys, clicks and controllers between presents. With V-Sync (the default on a
fixed-refresh display) the present waits for most of a refresh, so a key press or button waited up
to 16.7 ms at 60 Hz (about 8 on average), and SDL read the controllers only when the loop pumped
events: a stick's moves reached the PC bunched once per refresh instead of each millisecond.

While the loop waits in the present it changes nothing, so it lends the filter what it decides with
for that time (`DirectMotion::presenting`, `DirectMotion::lend`): whether keys are the PC's (captured,
focused, menu hidden) and the settings that map them, whether clicks are (the same with a relative
pointer), whether controllers send, and its controllers themselves. Meanwhile a thread calls
`SDL_UpdateJoysticks` every millisecond, as SDL does while the loop waits for events, so controller
events go through the filter on that thread; `Gamepads::handle` turns them into what the PC gets,
the same code the loop runs. During a stream Windows reads the keyboard on SDL's raw input thread
(`SDL_HINT_WINDOWS_RAW_KEYBOARD`), where clicks of a captured mouse already come from.

- No double sends: a key, click or wheel turn the filter sent still reaches the loop, marked, so the
  loop keeps track of held keys and buttons (to let them go when focus leaves) and sends nothing. A
  controller event the filter applied goes no further, unless it asks for something (a quit or menu
  chord, the controller mouse switch): then the loop does that and nothing else.
- Order: whatever the loop sends or decides on itself holds back the keys, clicks and controller
  events after it (a stream shortcut, a key from before the present, a new controller, a chord's
  action), so Shift held before the present still reaches the PC before a click during it, and a
  button after a menu chord goes to the menu. Motion only waits for clicks, as before.
- The UI is unchanged: outside the present, and with the menu open, everything goes through the
  loop; stream shortcuts, F12, Escape and B in the menu, holding Guide and the quit combo work as
  they did.
- Locks: the filter may run under SDL's joystick lock, so it only tries the lent state's lock and
  leaves the event to the loop when it is taken (while the loop draws with its controllers).

## Measured

2026-10-31 and 11-01, laptop on Wi-Fi 6E (RZ616, AC power) streaming the desktop from the RX 7900
XT PC with V-Sync on and the latency frame pacing. A script moved the mouse 2 px every 1 ms for 25 s
(1000 Hz, direction flipping every 500 ms; the 8 kHz rows 1 px every 125 us, flipping every 100 ms);
the PC's pointer was sampled at the PC and the laptop's uplink captured with pktmon. One value per
run.

| arm | PC pointer updates/s | gap p90 | per-frame motion CV | laptop packets/s (gap p90) |
|---|---|---|---|---|
| before | 185 | 13.6 ms | 0.94 | 553 (4.8 ms) |
| Wi-Fi mode only | 126 | 16.9 ms | 1.01 | 504 (5.7 ms) |
| direct motion only, no floor | 90, 402 | stalls to 90 ms, 5.9 ms | –, 0.62 | 1097, 1106 (1.04 ms) |
| both, no floor | 682, 511, 576 | 2.9, 4.0, 3.5 ms | 0.30, 0.49, 0.41 | 1109 to 1143 (1.04 ms) |
| both, 1 ms floor | 77 | stalls to 110 ms | – | 959 (1.6 ms) |
| both, 0.5 ms floor (shipped) | 817, 657 | 2.1, 3.3 ms | 0.22, 0.26 | 1067, 1117 (1.03 ms) |
| 8 kHz, both, 1 ms floor | 277 | 9.2 ms | 0.82 | 1188 (1.1 ms) |
| 8 kHz, both, 0.5 ms floor | 913 | 2.2 ms | 0.41 | 1952 (0.63 ms) |

Every arm delivered the full distance. With direct motion the laptop sends one packet per
millisecond (an 8 kHz mouse about one per 0.5 ms), and in no run did its uplink have a gap over
20 ms. What the PC receives depends on the link: in 2 of 9 direct-motion runs (one with the Wi-Fi
mode, one without) the Wi-Fi delivered the input in clumps 50 to 110 ms apart while the PC was idle,
so a bad Wi-Fi moment still shows. With direct motion, runs with the Wi-Fi mode reached 817, 682,
657, 576, 511 and 77 updates/s, runs without it 90 and 402; alone it changes nothing while motion
still leaves once per refresh.
