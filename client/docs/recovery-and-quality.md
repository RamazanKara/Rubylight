# Stream recovery and automatic quality

[Documentation](index.md) · [Settings reference](settings.md) · [Video and latency](video-and-latency.md)

## Recovering a connection

**Settings → Streaming → Recover interrupted streams** is on by default (an install that stored the old default, off, is switched on once; turning it off again is kept). When it is on, after a temporary transport failure, a normal game stream retries after 1, 2, 4, 8 and 12 seconds, with at most five attempts and a one-minute budget including connection startup. The progress dialog shows the attempt; Back cancels. Leaving the stream cancels pending recovery. A connection must remain up for 30 seconds before another interruption receives a fresh retry budget.

When an established game stream stops because the screen locks or the app leaves the foreground, returning can resume that captured game through the same bounded policy. No connection starts while the app is in the background. Time spent locked does not count as a stable connection or refresh an exhausted retry budget. Leaving during startup, a queued retry or teardown cancels that handoff and keeps recovery manual. With **Recover interrupted streams** off, leaving the stream ends it and there is no automatic recovery after screen lock.

Automatic recovery only resumes the same running game. It never sends a launch or quit request, confirms replacement, or takes over someone else's session. It stops if the game is no longer running, its identity changes, access is denied, or pairing is missing. Normal streams and a main-session Resume tile can recover. Remote Monitor, Remote Input and control actions keep the existing manual reconnect path. These restrictions apply with every Moonlight-compatible host.

Normal host termination closes the stream. Protected content, host frame-conversion errors, decoder startup failures and unknown errors do not trigger automatic recovery. The error dialog retains **Reconnect** and **Disconnect** after recovery stops. Manual Reconnect is an explicit new connection attempt using normal launch behavior. Recovery recreates the stream activity after the previous native connection has stopped, restoring its configured audio, controller and display setup; it does not quit the game.

Android 0.4.10 fixes a shutdown race where continued controller or input traffic could replace the host's normal end-of-stream notification with a generic disconnect error. When the host ends an established stream normally after the game exits, the app returns to the library without an error or automatic reconnect. Actual connection failures and host-reported errors still use the recovery and error handling above.

Android 0.4.9 preserves microphone mute for the current session through automatic recovery, screen-lock resume, manual **Reconnect** and [**Video output** handoffs](controls.md#tablet-zoom-and-external-displays). This includes a USB controller microphone muted by the host, even if controller capture is lost during teardown. Unmute from the stream menu when ready; microphone streaming still requires its setting and Android permission. The mute choice does not change the microphone-streaming preference and is not saved for a separate new session.

## Choosing automatic quality

**Settings → Video → Lower resolution or frame rate when needed**, next to Device controls bitrate, is off by default. Choose the mode, then the minimum short edge and FPS, which appear once it is on. The selected stream resolution and FPS are the upper bounds; a floor above those bounds is clamped to the starting value. Aspect ratio and decoder alignment are preserved. For a stream set to follow the native display, its ceiling follows fold, window and panel changes.

| Mode | Behavior |
| --- | --- |
| Preserve motion | Reduce resolution before lowering FPS. |
| Preserve detail | Reduce FPS before lowering resolution. |
| Extend battery life | Gradually reduce work even on a healthy connection, toward at most 60 FPS and a 1080-pixel short edge, while respecting the minimums. Further pressure can lower it more. |

During a stream, **stream menu → Quality → Automatic stream quality** overrides the mode for that PC. It appears where the host can change the stream in place. **Use app settings** removes that override. Turning it off restores the stream ceiling. Changing the mode starts its new policy from that ceiling. The floors come from app settings and apply to the next stream.

Automatic quality requires the host to advertise control message `0x5532` (currently Rubylight) and the active decoder to support in-place resolution changes. It is unavailable with PyroWave, input-only/remote-monitor streams, hosts without this capability, and non-adaptive decoders. The UI reports this fallback; it does not silently reconnect or change codecs. Automatic bitrate remains separately available where supported.

On a host that supports runtime bitrate (currently Rubylight), choosing automatic quality enables the existing bitrate controller first. The client reads the actual encoder budget and sends the current bitrate once to take control from host-side adaptation. It waits for sustained network trouble after substantial bitrate reduction before asking for a lower resolution/FPS. Slow decoding and thermal pressure can reduce quality independently because bitrate alone may not reduce those workloads.

Resolution/FPS reductions are at least 15 seconds apart. Decoder/thermal pressure must persist for four samples; network pressure at a reduced bitrate must persist for eight. Recovery takes one step after 30 healthy seconds, with adequate decode headroom and no thermal warning. Missing video, unknown recovery metrics, menus, backgrounding and picture in picture do not count as healthy time.

Decode timing allows two frame intervals for pipeline latency before treating a frame as late; recovery requires the average below one and a half intervals. A quiet host can recover while sending fewer frames than the requested FPS: each timing window still needs at least 12 fresh outputs spread over half a second, with its newest output no more than half a second old. Pauses, brief bursts and reused timing windows cannot count as recovery.

The quality request keeps the negotiated codec, HDR/color mode and host display selection. The host controls what display mode it can apply. If a requested size is rejected or has not appeared in decoded frames within ten seconds, automatic quality stops with a visible explanation and leaves the stream connected. FPS is shown as a requested target because this protocol has no separate frame-rate acknowledgement; received/rendered FPS in the existing overlay remains the measurement. A MediaCodec upscaler may fall back to direct output after a size change, as described in [upscaling](video-and-latency.md#upscaling).

## Automatic bitrate

Automatic bitrate sends one bitrate request per change through the same host path as a manual change; the host's own controller stands down while it is on.

- **Cuts must help.** Loss of 3% or more, or the native connection warning, cuts 20% every 3 seconds. Jitter or queueing alone also cuts, but each cut is checked: if jitter or queueing is not at least 15% lower after a cut than before it, and two cuts in a row did not help, the rate is held instead of ratcheted down. Holding ends after 5 healthy seconds, and loss clears it at once. Wi-Fi jitter that does not follow the bitrate therefore stops above the floor, while loss still cuts to the floor.
- **Fast recovery.** A rate with no poor sample for 10 seconds is remembered as stable. After trouble, once the link has been healthy for 5 seconds, the rate steps back toward the stable one by half the remaining gap (at least 250 kbps) every 3 seconds, so 2 to 50 Mbps takes about 30 seconds. Above the stable rate it probes by 5% every 15 seconds. If a step back brings poor samples again within 10 seconds, the rate returns to the one before it, the stable rate drops to halfway between the two, and the next try waits for 15 healthy seconds.
- **Floor.** The floor is 2 Mbps or the stream's pixels per second times a per-codec budget (H.264 0.10, HEVC 0.07, AV1 0.065 bits per pixel), whichever is higher: about 8.7 Mbps for 1080p60 HEVC, 2 Mbps for 720p30. It never exceeds half the starting bitrate and follows resolution and frame rate changes from Automatic stream quality. The ceiling is the video bitrate the stream started at.

## Verification limits

Policy tests cover retry budgets and exclusions, lifecycle cancellation, resume-only launch/quit/confirmation guards, mode ordering, floors, thermal/network hysteresis, delayed recovery and display changes. These checks do not measure real device latency, battery savings, roaming recovery or decoder/display compatibility. Those require a later hardware session; no live host, display or controller is needed for the unit suite.
