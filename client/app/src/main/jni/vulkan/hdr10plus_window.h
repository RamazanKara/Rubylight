#pragma once

// HDR10+ dynamic metadata on the output window, through the one entry point Android has for it:
// native_window_set_buffers_hdr10_plus_metadata, the call MediaCodec makes for its own output. The
// NDK has no setter for HDR10+ (only static SMPTE ST 2086 and CTA-861.3 metadata), so this calls
// ANativeWindow::perform with NATIVE_WINDOW_SET_BUFFERS_HDR10_PLUS_METADATA, as the platform's inline
// helper does. Every buffer queued after the call carries the blob, so it is set again only when the
// metadata changes.
//
// The ANativeWindow it calls perform on is native_window_mirror.h's. Kept free of Android headers
// so the host tests can run it.

#include <cstddef>
#include <cstdint>

#include "../window-dataspace/native_window_mirror.h"

namespace vkr {

using nwds::NativeWindowMirror;
using nwds::kNativeWindowMagic;

constexpr int kNativeWindowSetBuffersHdr10PlusMetadata = 34;

enum class Hdr10PlusResult {
    kUnchanged,    // Nothing was called: the same state, or the window refused before
    kSet,          // Buffers queued from now on carry the blob
    kCleared,      // They carry none
    kUnsupported,  // This window is not the struct mirrored here; said once, then never called
    kFailed,       // perform returned an error (error()); said once, then never called
};

class Hdr10PlusWindow {
public:
    // window: the ANativeWindow*, or null for none
    explicit Hdr10PlusWindow(void* window = nullptr) : window_(window) {}

    // Sets the blob (the HDR10+ SEI payload from itu_t_t35_country_code on) for the buffers queued
    // after this call; null or size 0 clears it, which calls nothing unless a blob was set.
    Hdr10PlusResult set(const uint8_t* data, size_t size) {
        if (refused_ || !window_) return Hdr10PlusResult::kUnchanged;
        if (!data || size == 0) {
            if (!holds_) return Hdr10PlusResult::kUnchanged;
            data = nullptr;
            size = 0;
        }
        auto* window = static_cast<NativeWindowMirror*>(window_);
        magic_ = window->common.magic;
        version_ = window->common.version;
        if (!nwds::isMirrored(window)) {
            refused_ = true;
            return Hdr10PlusResult::kUnsupported;
        }
        error_ = window->perform(window, kNativeWindowSetBuffersHdr10PlusMetadata, size, data);
        if (error_ != 0) {
            // -ENOENT: this Android does not know the operation
            refused_ = true;
            return Hdr10PlusResult::kFailed;
        }
        holds_ = size != 0;
        return holds_ ? Hdr10PlusResult::kSet : Hdr10PlusResult::kCleared;
    }

    // A blob is set on the window
    bool holds() const { return holds_; }
    // The window refused once (kUnsupported or kFailed), so nothing more is called
    bool refused() const { return refused_; }
    // perform's result after kFailed, such as -ENOENT
    int error() const { return error_; }
    // What the window's header held at the last call, for the log after kUnsupported
    int magic() const { return magic_; }
    int version() const { return version_; }

private:
    void* window_;
    bool holds_ = false;
    bool refused_ = false;
    int error_ = 0;
    int magic_ = 0;
    int version_ = 0;
};

}  // namespace vkr
