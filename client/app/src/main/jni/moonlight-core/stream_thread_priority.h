#pragma once

// The nice value latency.c gives a classic-engine stream thread as it names itself, the same rule
// as the Rust engine's priority::thread_level (client/protocol/client/src/priority.rs).

#include <stdbool.h>
#include <string.h>

// "Prioritize stream traffic": the receive, audio, control and input threads.
#define STREAM_THREAD_NICE (-4)
// Android's THREAD_PRIORITY_URGENT_DISPLAY, for the thread that calls the decoder; the caller
// falls back to STREAM_THREAD_NICE when it is refused.
#define DECODE_THREAD_NICE (-8)

// debug.nightjar.decode_priority: "0" turns the decode level off for the next stream, for A/B
// runs; anything else, or nothing, leaves it on.
static inline bool decodePriorityOn(const char* property) {
    return strcmp(property, "0") != 0;
}

// 0 leaves the thread alone. With direct submission VideoRecv runs the decoder, so it gets the
// decoder's level; without, the VideoDec thread does. Nothing is raised with "prioritize stream
// traffic" off.
static inline int streamThreadNice(const char* name, bool networkPriority, bool directSubmit,
                                   bool decodePriority) {
    if (!networkPriority) {
        return 0;
    }
    if (!strcmp(name, "VideoDec")) {
        return decodePriority ? DECODE_THREAD_NICE : 0;
    }
    if (!strcmp(name, "VideoRecv")) {
        return decodePriority && directSubmit ? DECODE_THREAD_NICE : STREAM_THREAD_NICE;
    }
    if (!strcmp(name, "AudioRecv") || !strcmp(name, "AudioDec") || !strcmp(name, "ControlRecv") ||
            !strcmp(name, "InputSend")) {
        return STREAM_THREAD_NICE;
    }
    return 0;
}
