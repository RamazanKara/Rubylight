# AMD AV1 frame skipping
- **AV1 on AMD no longer skips frames to stay on budget.** AMD's rate control could drop a frame when the bitrate got tight, which a VRR client shows as a held picture. H.264 and HEVC already kept every frame; AV1 now asks the driver for the same, and a driver without the setting keeps its default and says so in the log.
