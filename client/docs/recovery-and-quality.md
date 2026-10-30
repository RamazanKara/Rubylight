# Stream recovery and automatic quality

[Documentation](index.md) · [Settings reference](settings.md) · [Video and latency](video-and-latency.md)

Nightjar keeps a stream running through transient failures and adapts its quality to the link and the device. This page describes the rules: when a stream reconnects, when it lowers resolution or frame rate, and how the bitrate controller decides.

## Recovering a connection

**Settings → Streaming → Recover interrupted streams** is on by default. After a temporary transport failure, a game stream reconnects to the same running game:

| Rule | Value |
| --- | --- |
| Attempts | Up to five, after 1, 2, 4, 8 and 12 seconds |
| Budget | One minute, including connection startup |
| Fresh budget | After the connection has stayed up for 30 seconds |
| Cancel | Back in the progress dialog, or leaving the stream |

Recovery only resumes the game that was running. It never sends a launch or quit request, never confirms replacing a game, and never takes over another device's session; it stops if the game is no longer running, its identity has changed, access is denied or the pairing is gone. Normal streams and a Resume tile recover. Remote Monitor, Input-only and host actions keep the manual **Reconnect**. These rules hold with every Moonlight-compatible host.

A normal end of the stream by the host, protected content, host frame-conversion errors, decoder startup failures and unknown errors do not trigger recovery. When the host ends the stream because the game exited, Nightjar returns to the library without an error. After recovery stops, the error dialog offers **Reconnect**, which starts a new connection with normal launch behavior, and **Disconnect**.

### When the network changes

When the network disappears (Wi-Fi turned off or dropped), the stream shows "Network lost" over the picture at once, even with on-screen warnings off, and says that it will reconnect when an automatic reconnect will follow.

When a network returns, the stream continues in place where it can: after a [client roam](protocol/client-roaming.md), on the same address, or while there is no route yet, in which case the connection's own timeout decides. If the route to the PC now leaves from a different address that the connection cannot continue on, Nightjar reconnects to the same game immediately rather than waiting for the timeout; the immediate attempt still counts toward the budget. A network whose address arrives late, such as IPv6 before DHCP, is checked again each time its addresses change, up to the 10-second timeout.

Immediate reconnects do not happen on mobile data without **Continue on mobile data**, or when **Ride out network drops** is set above the 10-second timeout, since the tolerance then decides. Every attempt first waits while the PC has no route, checking every half second, instead of failing and backing off. If the minute ends without a route, the reconnect dialog says so.

Nightjar for Windows and Linux follows the same rules, checking the route to the PC once a second. With its default 3-second interruption grace, it also reconnects immediately, while its window is not minimized.

### Leaving and returning

When the screen locks or Nightjar leaves the foreground, the stream stops; returning resumes the same game. No connection starts while the app is in the background.

- A return after your own trip away (Home, another app, a task switch, a lock, or unplugging an external display while away) is not a retry: it uses no attempt, starts after one second and has its own minute to connect.
- A connection lost while away, or a failure while resuming, counts as a retry and uses the budget.
- Time away does not count toward the 30 seconds that earn a fresh budget, and does not refresh an exhausted one.
- With **Recover interrupted streams** off, leaving the stream ends it.

The microphone's mute state carries over automatic recovery, screen-lock resume, manual **Reconnect** and [video output changes](controls.md#tablet-zoom-and-external-displays) within a session, including a USB controller microphone muted by the host. It does not change the microphone setting and is not kept for a new session.

## Choosing automatic quality

**Settings → Video → Lower resolution or frame rate when needed** is off by default. When it is on, choose a mode and the minimum short edge and frame rate. The stream's resolution and frame rate are the upper bounds; a minimum above them is clamped. Aspect ratio and decoder alignment are preserved, and a stream that follows the native display follows fold, window and panel changes.

| Mode | Behavior |
| --- | --- |
| Preserve motion | Lowers resolution before frame rate. |
| Preserve detail | Lowers frame rate before resolution. |
| Extend battery life | Reduces work even on a healthy connection, toward at most 60 fps and a 1080-pixel short edge, within the minimums. Further pressure lowers it more. |

During a stream, **stream menu → Quality → Automatic stream quality** overrides the mode for that PC; **Use app settings** removes the override. The minimums come from the app settings and apply from the next stream.

### Requirements

Automatic quality needs a host that can change a running stream's resolution and frame rate (Rubylight 3.0) and a decoder that supports in-place resolution changes. It is unavailable with PyroWave and for Input-only and Remote Monitor streams. Where it is unavailable, the settings say why; Nightjar never reconnects or changes codec to apply it.

The change keeps the negotiated codec, HDR and color mode, and the host's display selection. If the host rejects a size, or the size has not appeared in decoded frames within ten seconds, automatic quality stops with an explanation and the stream stays connected. The frame rate is a requested target, since the protocol has no frame-rate acknowledgement; the overlay's received and rendered frame rates show what arrives. After a size change, a hardware-decoder upscaler can fall back to direct output, as described in [upscaling](video-and-latency.md#upscaling).

### How it decides

With a host that supports bitrate changes during a stream, automatic quality first turns on **Device controls bitrate**. It reads the encoder's actual bitrate, takes control from the host's own adaptation, and asks for a lower resolution or frame rate only after network trouble persists at a substantially reduced bitrate. Slow decoding and thermal pressure lower quality directly, since a lower bitrate may not reduce those loads.

| Rule | Value |
| --- | --- |
| Time between reductions | At least 15 seconds |
| Decoder or thermal pressure | Must persist for four samples |
| Network pressure at a reduced bitrate | Must persist for eight samples |
| Step back up | One step after 30 healthy seconds with decode headroom and no thermal warning |
| Late frame | Decode longer than two frame intervals; recovery needs an average below one and a half |

Missing video, unknown metrics, open menus, background time and picture-in-picture do not count as healthy time. Each timing window needs at least 12 fresh frames spread over half a second, the newest no more than half a second old, so pauses and short bursts cannot count as recovery.

## Device controls bitrate

**Device controls bitrate** sends one bitrate request per change, through the same path as a manual change; the host's own controller stands down while it is on.

- **Cuts must help.** Loss of 3% or more, or the connection's own warning, cuts the bitrate by 20% every 3 seconds. Jitter or queueing alone also cuts, but if two cuts in a row did not lower jitter or queueing by at least 15%, the rate is held instead of cut further. Holding ends after 5 healthy seconds; loss ends it at once. Wi-Fi jitter that does not depend on the bitrate therefore stops above the floor, while loss still cuts to the floor.
- **Fast recovery.** A rate with no poor sample for 10 seconds is remembered as stable. After trouble, once the link has been healthy for 5 seconds, the rate climbs back toward the stable rate by half the remaining gap (at least 250 kbps) every 3 seconds, so 2 to 50 Mbps takes about 30 seconds. Above the stable rate it probes up by 5% every 15 seconds. If a step brings poor samples back within 10 seconds, the rate returns to the previous one, the stable rate drops halfway between the two, and the next attempt waits for 15 healthy seconds.
- **Floor and ceiling.** The floor is the higher of 2 Mbps and the stream's pixels per second times a per-codec budget (H.264 0.10, HEVC 0.07, AV1 0.065 bits per pixel): about 8.7 Mbps for 1080p60 HEVC, 2 Mbps for 720p30. It never exceeds half the starting bitrate and follows resolution and frame rate changes from automatic quality. The ceiling is the bitrate the stream started at.
