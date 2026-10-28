# Scene cuts no longer freeze the picture
- When a scene cut comes out of the AMD encoder too large to keep its FEC, the host now encodes that picture again as a keyframe, which fits, and sends that instead. In our 250 Mbps HEVC 1440p60 test with 0.3% loss, lost scene cuts went from 15–16 to 0 and the short freeze after each one (about 50 ms, 15 in 45 s) is gone, with no change in encode time.
