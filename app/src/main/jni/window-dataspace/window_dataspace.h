#pragma once

// The output window's dataspace, set through the public ANativeWindow_setBuffersDataSpace (API 28)
// and, where that refuses it, through the call the platform's own native_window_set_buffers_data_space
// makes: ANativeWindow::perform with NATIVE_WINDOW_SET_BUFFERS_DATASPACE. The public call first
// checks the dataspace against a list (android13-release libnativewindow isDataSpaceValid has no
// BT2020_HLG; android14-release has no list), perform goes straight to the window. Whether a display
// then takes the dataspace is the window's answer, asked per device (probeDataspace), never assumed
// from the Android version.
//
// The ANativeWindow it calls perform on is native_window_mirror.h's. Kept free of Android headers
// so the host tests can run it.

#include <cstdint>

#include "native_window_mirror.h"

namespace nwds {

constexpr int kNativeWindowSetBuffersDataspace = 19;

// ADATASPACE_BT2020_HLG and ADATASPACE_UNKNOWN
constexpr int32_t kDataspaceBt2020Hlg = 168165376;
constexpr int32_t kDataspaceUnknown = 0;

// Results that are never a platform's (those are 0 or a negative errno)
constexpr int kNotCalled = 1;
constexpr int kMissing = 2;      // ANativeWindow_setBuffersDataSpace was not found
constexpr int kNotMirrored = 3;  // the window is not the struct mirrored here; perform not called

// Which call took the dataspace; the values are WindowDataspace.java's
enum class DataspaceCall : int { kNone = 0, kPublic = 1, kPerform = 2 };

struct DataspaceOutcome {
    DataspaceCall call = DataspaceCall::kNone;
    int publicResult = kNotCalled;   // 0, a negative errno or kMissing
    int performResult = kNotCalled;  // 0, a negative errno, kNotMirrored or kNotCalled
    bool accepted() const { return call != DataspaceCall::kNone; }
};

// native_window_set_buffers_data_space: 0 or perform's error, kNotMirrored for another layout
inline int performSetDataspace(void* window, int32_t dataspace) {
    auto* mirror = static_cast<NativeWindowMirror*>(window);
    if (!isMirrored(mirror)) return kNotMirrored;
    return mirror->perform(mirror, kNativeWindowSetBuffersDataspace, dataspace);
}

// Sets the dataspace with set(window, dataspace), ANativeWindow_setBuffersDataSpace or kMissing,
// then, only where that refused it and withPerform is on, through perform.
template <typename Set>
DataspaceOutcome setDataspace(void* window, int32_t dataspace, Set set, bool withPerform) {
    DataspaceOutcome outcome;
    outcome.publicResult = set(window, dataspace);
    if (outcome.publicResult == 0) {
        outcome.call = DataspaceCall::kPublic;
        return outcome;
    }
    if (!withPerform) return outcome;
    outcome.performResult = performSetDataspace(window, dataspace);
    if (outcome.performResult == 0) outcome.call = DataspaceCall::kPerform;
    return outcome;
}

struct DataspaceProbe {
    DataspaceOutcome set;          // the dataspace asked about
    int32_t previous = kDataspaceUnknown;
    bool previousKnown = false;    // get answered; else UNKNOWN, a new Surface's own, is restored
    DataspaceOutcome restore;      // only called when set was accepted
};

// Whether this window takes the dataspace, leaving it as it was: get(window) is
// ANativeWindow_getBuffersDataSpace (the dataspace or a negative errno), set as for setDataspace.
// The previous dataspace goes back through the public call, else perform.
template <typename Set, typename Get>
DataspaceProbe probeDataspace(void* window, int32_t dataspace, Set set, Get get) {
    DataspaceProbe probe;
    const int32_t previous = get(window);
    probe.previousKnown = previous >= 0;
    probe.previous = probe.previousKnown ? previous : kDataspaceUnknown;
    probe.set = setDataspace(window, dataspace, set, true);
    if (probe.set.accepted()) probe.restore = setDataspace(window, probe.previous, set, true);
    return probe;
}

}  // namespace nwds
