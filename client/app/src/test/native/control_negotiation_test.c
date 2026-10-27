// gcc -std=c11 -O2 app/src/test/native/control_negotiation_test.c -o /tmp/control-negotiation-test
#include <stdio.h>
#include "../../main/jni/moonlight-core/control_negotiation.h"

static int failures;

static void expect(const char* name, const char* sdp, const uint16_t* want, int wantCount) {
    uint16_t ids[RL_MAX_CONTROL_MESSAGES];
    int count = rlParseControlMessages(sdp, ids, RL_MAX_CONTROL_MESSAGES);
    int ok = count == wantCount;
    for (int i = 0; ok && i < count; i++) ok = ids[i] == want[i];
    if (!ok) {
        failures++;
        printf("FAIL %s: got %d ids:", name, count);
        for (int i = 0; i < count; i++) printf(" 0x%04x", ids[i]);
        printf("\n");
    }
}

int main(void) {
    const uint16_t rubylight[] = { 0x5530, 0x5531, 0x5532 };
    expect("rubylight 2.2.0",
           "v=0\r\na=x-ss-general.featureFlags:3\r\na=x-rl-control:0x5530,0x5531,0x5532\r\n"
           "a=rtpmap:96 opus/48000/1\r\n", rubylight, 3);
    expect("last line without newline", "v=0\na=x-rl-control:0x5530,0x5531,0x5532", rubylight, 3);
    expect("spaces, case and decimal", "a=x-rl-control: 0X5530 ,21809, 0x5532 \r\n", rubylight, 3);
    expect("no line", "v=0\r\na=x-ss-general.featureFlags:3\r\n", NULL, 0);
    expect("empty value", "a=x-rl-control:\r\n", NULL, 0);
    expect("null sdp", NULL, NULL, 0);
    const uint16_t phase[] = { 0x5530 };
    expect("malformed entries skipped", "a=x-rl-control:0x5530,0x10000,0xzz,,0x,-1\r\n", phase, 1);
    expect("duplicates dropped", "a=x-rl-control:0x5530,0x5530\r\na=x-rl-control:0x5530\r\n", phase, 1);
    expect("prefix must start the line", "a=x-rl-controls:0x5531\r\nb=a=x-rl-control:0x5531\r\n", NULL, 0);
    const uint16_t two[] = { 0x5530, 0x5531 };
    expect("several lines", "a=x-rl-control:0x5530\r\na=x-rl-control:0x5531\r\n", two, 2);

    // More ids than fit are dropped, never written past the buffer.
    uint16_t small[2];
    if (rlParseControlMessages("a=x-rl-control:1,2,3,4\n", small, 2) != 2 || small[0] != 1 || small[1] != 2) {
        failures++;
        printf("FAIL capacity\n");
    }

    if (failures) return 1;
    printf("control negotiation: all tests passed\n");
    return 0;
}
