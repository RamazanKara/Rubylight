#pragma once

// The output window's dataspace, set through the public ANativeWindow_setBuffersDataSpace (API 28)
// and, where that refuses it, through the call the platform's own native_window_set_buffers_data_space
// makes: ANativeWindow::perform with NATIVE_WINDOW_SET_BUFFERS_DATASPACE. The public call first
// checks the dataspace against a list (android13-release libnativewindow isDataSpaceValid has no
// BT2020_HLG; android14-release has no list), perform goes straight to the window. Whether a display
// then takes the dataspace is the window's answer, asked per device (probeDataspace), never assumed
// from the Android version.
//
// The struct below mirrors struct ANativeWindow from the platform's system/window.h
// (android14-release libs/nativewindow/include/system/window.h 343-470, unchanged in main), which
// the NDK does not ship. Its first two fields are a magic number and the struct's own size, which
// the platform sets in ANativeWindow's constructor: perform is only called when both match this
// mirror. Kept free of Android headers so the host tests can run it.

#include <cstddef>
#include <cstdint>

namespace nwds {

// android_native_base_t
struct NativeBaseMirror {
    int magic;
    int version;
    void* reserved[4];
    void (*incRef)(NativeBaseMirror* base);
    void (*decRef)(NativeBaseMirror* base);
};

// struct ANativeWindow, to its last member
struct NativeWindowMirror {
    NativeBaseMirror common;
    uint32_t flags;
    int minSwapInterval;
    int maxSwapInterval;
    float xdpi;
    float ydpi;
    intptr_t oem[4];
    int (*setSwapInterval)(NativeWindowMirror* window, int interval);
    int (*dequeueBuffer_DEPRECATED)(NativeWindowMirror* window, void** buffer);
    int (*lockBuffer_DEPRECATED)(NativeWindowMirror* window, void* buffer);
    int (*queueBuffer_DEPRECATED)(NativeWindowMirror* window, void* buffer);
    int (*query)(const NativeWindowMirror* window, int what, int* value);
    int (*perform)(NativeWindowMirror* window, int operation, ...);
    int (*cancelBuffer_DEPRECATED)(NativeWindowMirror* window, void* buffer);
    int (*dequeueBuffer)(NativeWindowMirror* window, void** buffer, int* fenceFd);
    int (*queueBuffer)(NativeWindowMirror* window, void* buffer, int fenceFd);
    int (*cancelBuffer)(NativeWindowMirror* window, void* buffer, int fenceFd);
};

// The platform's sizes on its 64-bit and 32-bit ABIs
static_assert(sizeof(NativeWindowMirror) == (sizeof(void*) == 8 ? 192 : 108), "ANativeWindow mirror");

// ANDROID_NATIVE_MAKE_CONSTANT('_','w','n','d')
constexpr int kNativeWindowMagic = ('_' << 24) | ('w' << 16) | ('n' << 8) | 'd';
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
    if (!mirror || mirror->common.magic != kNativeWindowMagic ||
        mirror->common.version != static_cast<int>(sizeof(NativeWindowMirror)) || !mirror->perform) {
        return kNotMirrored;
    }
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
