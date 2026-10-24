#include "li_compat.h"

// Hidden, so it never clashes with a definition in moonlight-core if that gains the call.
extern "C" __attribute__((visibility("hidden"))) bool LiGetHostTimeOnLocalClock(uint64_t, uint64_t*) {
    return false;
}
