#pragma once

// The one mirror of struct ANativeWindow, for the calls the NDK has no function for: the window
// dataspace through perform (window_dataspace.h) and HDR10+ metadata (vulkan/hdr10plus_window.h).
//
// The struct below mirrors struct ANativeWindow from the platform's system/window.h
// (android14-release libs/nativewindow/include/system/window.h 343-470, unchanged in main), which
// the NDK does not ship. Its first two fields are a magic number and the struct's own size, which
// the platform sets in ANativeWindow's constructor: perform is only called when both match this
// mirror (isMirrored). Kept free of Android headers so the host tests can run it.

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

// The window is the struct mirrored here and has a perform to call
inline bool isMirrored(const NativeWindowMirror* window) {
    return window && window->common.magic == kNativeWindowMagic &&
           window->common.version == static_cast<int>(sizeof(NativeWindowMirror)) && window->perform;
}

}  // namespace nwds
