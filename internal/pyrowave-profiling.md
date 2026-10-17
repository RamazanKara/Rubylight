# PyroWave submission profiling

The overlay's CPU submission time is elapsed wall time around the native submission
call. It can include parsing, command recording, driver work, and waits. It is not
GPU completion time or exclusively time scheduled on a CPU.

To separate those costs on a connected device, enable the opt-in native counters
before starting a stream:

```sh
adb shell setprop debug.nightjar.pyro_submit 1
adb logcat -s PyroWave
```

Restart the stream after changing the property. A release build can collect these
counters without making the app debuggable or changing its data. Profiling is off
by default. Turn it off after testing:

```sh
adb shell setprop debug.nightjar.pyro_submit 0
```

Each five-second window reports frame counts, accepted/busy/rejected submissions,
average and maximum input bytes, record-framed versus ordinary frame counts, and
the submitting thread's nice value. Stage lines report their own sample count and
average/maximum milliseconds for both monotonic wall time and thread CPU time:

| Stage | Work included |
| --- | --- |
| `copy` | Copy the Java input array into the reusable native buffer. |
| `clock` | Publish the measured display clock to the presenter. |
| `parse` | Validate/reconstruct records, feed packets to the decoder, and check readiness. |
| `slot` | Reserve output planes, polling existing draw fences. |
| `order` | Submit the dependency on the preceding GPU decode. |
| `sdk` | Record and submit the decode through the SDK, including any SDK retirement waits. |
| `publish` | Publish the submitted slot to the completion worker. |
| `stats` | Existing periodic decoder statistics. |
| `total` | The complete native submission call, including early failures. |

Warmup is excluded. Stages after a rejected or busy frame may have fewer samples
than `total`; use the counts when combining windows. Total excludes the diagnostic
log writes themselves, while the unchanged Java overlay timer includes them.
Enabled profiling adds clock reads, so compare builds with the same profiling
setting and confirm the final overlay with profiling off.

Use the same device, driver, input geometry, bit depth, scene, actual received
frame rate, and settings for comparisons. A static host may reduce its output
rate, allowing the phone to lower CPU clocks even when 120 FPS is selected.
Discard startup windows and check for thermal changes and frame replacement.
Perfetto scheduler traces distinguish runnable delay from CPU execution; these
stage counters then locate work or waits inside submission. Neither method alone
measures physical input-to-photon latency.

The same opt-in property also reports presentation and pacing windows using
timestamps already collected by the renderer. These counters do not read extra
clocks or change presentation decisions:

- `present profile` pairs each frame's decode completion, release, and actual
  presentation feedback. It separates time before release from time after it;
  only frames with matching valid feedback contribute to the averages.
- `pacing profile` reports the measured display period, learned submission lead,
  unrounded host-clock mapping relative to completion, target offset, and requested
  wait. Requested wait is a scheduling decision, not a measured sleep duration.
- `swapchain profile` records requested and actual image counts, transform, extent,
  format and presentation mode at creation. An Android driver may allocate more
  images than requested; image count alone does not measure queued frames.

Actual presentation feedback is a driver/compositor timestamp, not an independent
measurement of physical scanout. Use a graphics trace to locate compositor work
and retain the same cohort when combining stage averages. Balanced pacing also
recovers a stale host/local offset after eight consecutive early selected frames;
short bursts retain their original cadence, and Lowest latency does not use this
recovery path.
