# Real VRR

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/real-vrr/chart-dark.png">
    <img src="assets/real-vrr/chart-light.png" width="900" alt="Chart: on the same PC, 2.5 to 3.1% of frames land more than 1 ms off the game's rhythm with Real VRR instead of 10.2 to 19.4% shown on arrival, for 1.3 to 1.5 ms of delay. Over Wi-Fi, 29 to 31% instead of 64 to 74%, for 2.6 to 4.2 ms.">
  </picture>
</p>

With **Variable refresh rate** on, the screen refreshes whenever Nightjar shows a frame. Shown the
moment each one is decoded, frames reach the screen in the network's rhythm, not the game's: a game
running a steady 90 fps can arrive 8, 14, 9 and 13 ms apart, because packets never arrive perfectly
evenly. **Real VRR** shows each frame at the moment the game presented it on the PC, plus a short,
steady delay, so the screen refreshes in step with the game.

- **97% of frames on the game's rhythm**, instead of 81 to 90% shown on arrival, for 1.3 to 1.5 ms
  of delay on a clean link (PyroWave, HEVC and AV1 alike).
- **Built for Wi-Fi too.** A frame much later than the rest is shown when it is ready instead of
  holding every frame back for it, so over Wi-Fi it halves the frames off the rhythm for 2.6 to
  4.2 ms.
- **On by default** whenever Variable refresh rate is on.

## Turning it on or off

Real VRR is in **Settings → Display & image**, among the refresh rate settings (under **More
settings**), on Android and the desktop. It shows while **Variable refresh rate** is on and is on
by default. Settings saved before it became the default have it turned on once; if you turn it off
afterwards, it stays off. Turn it off for the lowest latency, when you would rather see each frame
the moment it is decoded.

What it needs:

- **Windows and Linux:** a display with G-Sync, FreeSync or HDMI VRR turned on, so that Variable
  refresh rate applies. If the display turns out not to refresh when a frame is presented, Nightjar
  waits for the refresh instead, as without VRR, and the overlay says so.
- **Android:** Variable refresh rate, which needs Android 16 or newer on a screen with adaptive
  refresh (ARR).
- **A PC:** any Moonlight-compatible host sends a timestamp with every frame, and Real VRR follows
  it. Rubylight stamps each frame with the moment the game presented it; other hosts' stamps follow
  the game as closely as their capture does.

On Android with Variable refresh rate off, the matching setting is **Show frames at the PC's
frame times** (off by default): it shows each frame on the refresh nearest its time on the PC. See
[refresh rate and VRR](video-and-latency.md#refresh-rate-and-vrr).

## How it works

**On the PC.** For a VRR stream to a virtual display, Rubylight runs that display at 1000 Hz
(unless its **Virtual display refresh** is Off), so each frame is captured the moment the game
draws it rather than on the next refresh of a 120 Hz display. With automatic capture it uses
Windows Graphics Capture and
stamps each frame with the moment the game called present, which it reads from DXGI's event
trace; where that trace is not available, it uses the time Windows composed the frame. That stamp
is the frame's RTP timestamp, so nothing in the stream changes for other clients.

**In Nightjar.** For every frame, Nightjar compares when the frame was ready to show with its time
on the PC. It needs no shared clock: the offset between the two clocks is the same for every frame,
so it cancels out, and slow drift is followed as it goes. From the last 128 frames it takes how late
97% of them were ready, adds 0.5 ms for the present call itself, and shows each frame at its PC time
plus that delay. The delay rises at once when frames get later, and falls back gradually when they
get earlier again, so a single stall is forgotten within seconds.

Two rules keep that delay short:

- **Stragglers don't hold everyone back.** The delay is never more than 2 ms above the typical
  (median) frame's lateness. A frame later than that is shown when it is ready. On a clean link the
  bound never applies; on Wi-Fi, where a few frames always arrive much later than the rest, it is
  what keeps the delay at 2.6 to 4.2 ms instead of 8 to 10 ms.
- **The screen's limit is respected.** Two frames are never shown closer together than the
  display's fastest refresh, and a frame is pushed back by at most one refresh for it.

On the desktop, the overlay's **Pacing** line reads **VRR, PC frame times**, with the average hold
and how many frames came later than the delay.

## Measured

A test animation runs at an uneven, game-like frame rate and is streamed at 1080p and 120 fps. A
frame is counted as off the rhythm when the gap before it is more than 1 ms away from the
animation's own timing, measured at the moment Nightjar presents it, which is when a VRR screen
shows it. Each figure is two 45-second runs.

**On the same PC** (no network in between, frame gaps of 8, 12, 8, 17 and 12 ms):

| Frames more than 1 ms off the game's rhythm | Shown on arrival | Real VRR | Delay Real VRR adds (median) |
|---|---|---|---|
| PyroWave | 10.2% (one run) | 2.5% | 1.5 ms |
| HEVC | 19.4% | 2.9% | 1.4 ms |
| AV1 | 15.5% | 3.1% | 1.3 ms |

The result is steadier than the PC's own capture timestamps, which vary by a millisecond or two. On
a 240 Hz monitor with VRR turned on, Real VRR added the same 1.3 to 1.4 ms. (That run cannot judge
the rhythm: the test animation and Nightjar share one screen there, and Nightjar's own presents set
when that screen refreshes.)

**Over Wi-Fi** (a laptop streams to the PC, frame gaps of 8, 17, 8, 25 and 17 ms):

| Frames off the rhythm, delay added | Shown on arrival | Real VRR without the 2 ms bound | Real VRR |
|---|---|---|---|
| PyroWave | 64 to 73% | 25%, 10.0 ms | 31%, 4.2 ms |
| HEVC | 68 to 69% | 20%, 8.0 ms | 29%, 2.6 ms |
| AV1 | 74% | 22%, 8.2 ms | 30%, 3.0 ms |

Part of the share left over Wi-Fi is the test laptop itself, whose capture timestamps are rough.
The method, the raw figures and the replays behind the 2 ms bound are in
[PyroWave decoding: Real VRR](pyrowave-decode.md#real-vrr-measured-cadence-and-a-link-with-a-long-tail).

## Limits

- **It adds delay:** about 1.3 to 1.5 ms on a clean link and 2.6 to 4.2 ms over Wi-Fi in the runs
  above. That is the price of the steady rhythm; turn Real VRR off if you would rather have each
  frame the moment it is decoded.
- **Over Wi-Fi some frames stay off the rhythm.** A frame more than 2 ms later than the typical one
  is shown when it is ready, so about 30% of frames were still off in the runs above, against 64 to
  74% shown on arrival.
- **Frames are shown on arrival** for PyroWave streams on Android, and on Windows when **Sharper
  colored text** streams packed full chroma, which it does when the codec's own 4:4:4 is not
  available on the device or the PC.
- The rhythm can only be as even as the PC's timestamps. With Rubylight and automatic capture they
  are the game's own present times; a capture method other than Windows Graphics Capture stamps the
  time each frame was captured.
