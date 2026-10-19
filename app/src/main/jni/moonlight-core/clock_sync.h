// Clock sync (control message 0x5537, protocol/core/src/control.rs): the host answers each probe
// with its receive and send times on the video RTP clock. android_present_clock.c feeds the
// replies to the timestamp presentation clock.
#pragma once
#include <stdint.h>

#define RL_CLOCK_SYNC_MESSAGE_TYPE 0x5537
#define RL_CLOCK_SYNC_REPLY_BYTES 32
// Newer hosts may append fields; anything this large is not a reply.
#define RL_CLOCK_SYNC_MAX_PAYLOAD 256

// Called on the control receive thread with a reply's payload; stamps its arrival first.
void BridgeClClockSync(const uint8_t* payload, int length);
