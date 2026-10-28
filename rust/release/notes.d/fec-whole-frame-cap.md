# Recovery keyframes keep their FEC
- After a scene cut is lost, the recovery keyframe now stays small enough to keep its full FEC within Moonlight's four blocks, so random packet loss on a fast wired route no longer loses it. In our 250 Mbps HEVC 1440p60 test with 0.3% loss, freezes over 100 ms went from 4 to 0, the slow pace and bitrate cuts no longer set in, and picture age dropped by 3 ms at p50 and p99.
- This covers a fixed FEC percentage and Nightjar's automatic FEC (with the percentage chosen for that frame). Without FEC the keyframe is held to the fewest packets four blocks allow.
