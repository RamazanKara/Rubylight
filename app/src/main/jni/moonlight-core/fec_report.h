// Nightjar's once-a-second video FEC loss report (0x5539, protocol/core/src/fec_report.rs).
#pragma once
#include <stdbool.h>
#include <stdint.h>

// Starts a new stream's tally.
void resetFecReports(void);
// One FEC block that completed or was given up, with the video queue's counters (the shards that
// arrived, the holes before the last arrival), like nightjar-core's video queue. Called on the
// video receive thread; android_control.c counts it for hosts that announced 0x5539.
void rlFecBlockFinished(uint32_t frame, uint8_t blockIndex, uint8_t blockCount, uint32_t dataShards,
                        uint32_t parityShards, uint32_t arrivedShards, uint32_t missingShards, bool unrecovered);

// Foundation Sunshine's client FEC control, ported from Moonlight V+ (qiin2333's moonlight-common-c
// 2aa14ff, VideoFecControlSupported): a Sunshine host whose DESCRIBE says
// x-ss-video[0].fecControlVersion:1, off PyroWave and without Rubylight's 0x5539, gets
// x-ml-video.fecPercentage in the ANNOUNCE and a 0x5510 summary every second
// (protocol/core/src/fec_report.rs, FecSummary).
extern bool RlFecSummariesEnabled;
// The choice the ANNOUNCE carries: -2 host, -1 automatic, 0-100 fixed.
extern int AndroidVideoFecPercentage;
void resetFecSummaries(void);
// connectionRecordFecBlock: a finished block's data shards and how many of them arrived. Each block
// counts once.
void rlFecSummaryBlock(uint32_t frame, uint8_t block, uint16_t data, uint16_t received);
