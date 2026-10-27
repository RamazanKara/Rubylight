#include "../../main/jni/moonlight-core/moonlight-common-c/src/ControlStream.c"

_Static_assert(sizeof(packetTypesGen3) / sizeof(packetTypesGen3[0]) > IDX_DS_ADAPTIVE_TRIGGERS,
               "Gen 3 callback lookup exceeds the packet table");
_Static_assert(sizeof(packetTypesGen4) / sizeof(packetTypesGen4[0]) > IDX_DS_ADAPTIVE_TRIGGERS,
               "Gen 4 callback lookup exceeds the packet table");
_Static_assert(sizeof(packetTypesGen5) / sizeof(packetTypesGen5[0]) > IDX_DS_ADAPTIVE_TRIGGERS,
               "Gen 5 callback lookup exceeds the packet table");
_Static_assert(sizeof(packetTypesGen7) / sizeof(packetTypesGen7[0]) > IDX_DS_ADAPTIVE_TRIGGERS,
               "Gen 7 callback lookup exceeds the packet table");
_Static_assert(sizeof(packetTypesGen7Enc) / sizeof(packetTypesGen7Enc[0]) > IDX_DS_ADAPTIVE_TRIGGERS,
               "Encrypted Gen 7 callback lookup exceeds the packet table");
