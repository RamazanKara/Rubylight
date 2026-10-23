#include "li_compat.h"

// Hidden, so they never clash with a definition in moonlight-core if it gains these calls.
#define LI_COMPAT __attribute__((visibility("hidden")))

extern "C" LI_COMPAT void LiSetPartialFrameDeadline(bool, int64_t, int) {}

extern "C" LI_COMPAT bool LiGetHostTimeOnLocalClock(uint64_t, uint64_t*) {
    return false;
}
