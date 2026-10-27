#include <assert.h>
#include <string.h>
#include "../../main/jni/moonlight-core/ControllerHaptics.h"

int main(void) {
    uint8_t packet[ML_HAPTICS_MAX_PAYLOAD] = {1, 1};
    MlHapticsWrite16(packet + 2, 15);
    MlHapticsWrite32(packet + 4, 0xffffffffu);
    MlHapticsWrite16(packet + 8, 240);
    assert(MlHapticsValidate(packet, sizeof(packet)));
    assert(MlHapticsRead32(packet + 4) == 0xffffffffu);
    assert(!MlHapticsValidate(NULL, sizeof(packet)));
    for (size_t size = 0; size < sizeof(packet); size++) assert(!MlHapticsValidate(packet, size));
    assert(!MlHapticsValidate(packet, sizeof(packet) + 1));
    const int offsets[] = {0, 1, 2, 8, 10};
    const uint8_t invalid[] = {2, 2, 16, 241, 1};
    for (size_t i = 0; i < sizeof(offsets) / sizeof(offsets[0]); i++) {
        uint8_t saved = packet[offsets[i]];
        packet[offsets[i]] = invalid[i];
        assert(!MlHapticsValidate(packet, sizeof(packet)));
        packet[offsets[i]] = saved;
    }
    MlHapticsWrite16(packet + 8, 0);
    assert(!MlHapticsValidate(packet, ML_HAPTICS_HEADER_SIZE));
    MlHapticsWrite16(packet + 8, 1);
    assert(MlHapticsValidate(packet, ML_HAPTICS_HEADER_SIZE + ML_HAPTICS_FRAME_BYTES));
    return 0;
}
