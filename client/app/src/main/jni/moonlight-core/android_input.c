#include "Limelight-internal.h"
#include "latency.h"

static void sleepInput(int ms) {
    // Only bypass the mouse/pen batching wait, never the keyboard synchronization delays.
    if (ms != 1 || !atomic_load(&AndroidUnbatchedInput)) {
        PltSleepMs(ms);
    }
}

// Keep the upstream submodule intact and scope the sleep override to its input sender.
#define PltSleepMs sleepInput
#include "moonlight-common-c/src/InputStream.c"
#undef PltSleepMs
