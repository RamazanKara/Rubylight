// gcc -std=c11 -O2 app/src/test/native/input_only_ping_test.c -o /tmp/input-only-ping-test
#undef NDEBUG
#include <assert.h>
#include <stdio.h>
#include "../../main/jni/moonlight-core/android_input_only_ping.h"

int main(void) {
    // Only an input-only session with a negotiated video port pings.
    assert(rlInputOnlyPingsVideo(true, 47998));
    assert(!rlInputOnlyPingsVideo(true, 0));
    assert(!rlInputOnlyPingsVideo(false, 47998));

    // The RTSP payload and a big-endian sequence, as VideoStream.c sends them.
    const uint8_t payload[RL_PING_PAYLOAD_SIZE] = "0123456789abcde";
    uint8_t out[RL_PING_MAX_SIZE];
    size_t length = rlBuildVideoPing(payload, 1, out);
    assert(length == RL_PING_MAX_SIZE);
    assert(memcmp(out, payload, RL_PING_PAYLOAD_SIZE) == 0);
    assert(out[16] == 0 && out[17] == 0 && out[18] == 0 && out[19] == 1);
    length = rlBuildVideoPing(payload, 0x01020304, out);
    assert(length == 20 && out[16] == 1 && out[17] == 2 && out[18] == 3 && out[19] == 4);

    // A host that sent no payload gets the legacy ping.
    const uint8_t none[RL_PING_PAYLOAD_SIZE] = { 0 };
    length = rlBuildVideoPing(none, 7, out);
    assert(length == 4 && memcmp(out, "PING", 4) == 0);

    printf("input_only_ping: passed\n");
    return 0;
}
