// Nightjar's once-a-second video FEC loss report (0x5539, protocol/core/src/fec_report.rs).
#pragma once
#include <stdbool.h>
#include <stdint.h>

// Starts a new stream's tally.
void resetFecReports(void);
// One FEC block that completed or was given up, counted like nightjar-core's video queue. Called
// on the video receive thread; android_control.c counts it for hosts that announced 0x5539.
void rlFecBlockFinished(uint32_t dataShards, uint32_t parityShards, uint32_t missingShards, bool unrecovered);
