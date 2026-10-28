// WindowDataspace.java's native side: asks a Surface's window whether it takes a dataspace
// (window_dataspace.h), for the renderers that draw HDR through the window dataspace.

#include <android/native_window_jni.h>
#include <dlfcn.h>
#include <jni.h>

#include "window_dataspace.h"

namespace {

// ANativeWindow_setBuffersDataSpace and ANativeWindow_getBuffersDataSpace are API 28 and the app's
// native code is built for API 21. dlsym(RTLD_DEFAULT) searches only the app's linker namespace,
// which holds neither library, so each is opened and searched by handle. They stay loaded.
constexpr const char* kLibraries[] = {"libnativewindow.so", "libandroid.so"};

void* findSymbol(const char* name) {
    for (const char* library : kLibraries) {
        void* handle = dlopen(library, RTLD_NOW | RTLD_LOCAL);
        if (handle == nullptr) continue;
        if (void* symbol = dlsym(handle, name)) return symbol;
    }
    return nullptr;
}

using SetDataSpace = int32_t (*)(ANativeWindow*, int32_t);
using GetDataSpace = int32_t (*)(ANativeWindow*);

}  // namespace

extern "C" {

// The call that took the dataspace (nwds::DataspaceCall); detail gets the public call's result,
// perform's, the previous dataspace (-1 when the window did not say), and the two results of
// putting it back.
JNIEXPORT jint JNICALL
Java_com_limelight_binding_video_WindowDataspace_nativeProbe(JNIEnv* env, jclass, jobject surface, jint dataspace,
                                                            jintArray detail) {
    if (surface == nullptr || detail == nullptr || env->GetArrayLength(detail) < 5) return 0;
    ANativeWindow* window = ANativeWindow_fromSurface(env, surface);
    if (window == nullptr) return 0;
    static const auto set = reinterpret_cast<SetDataSpace>(findSymbol("ANativeWindow_setBuffersDataSpace"));
    static const auto get = reinterpret_cast<GetDataSpace>(findSymbol("ANativeWindow_getBuffersDataSpace"));
    const nwds::DataspaceProbe probe = nwds::probeDataspace(
            window, dataspace,
            [](void* w, int32_t d) { return set != nullptr ? set(static_cast<ANativeWindow*>(w), d) : nwds::kMissing; },
            [](void* w) { return get != nullptr ? get(static_cast<ANativeWindow*>(w)) : -1; });
    ANativeWindow_release(window);
    const jint values[5] = {probe.set.publicResult, probe.set.performResult, probe.previousKnown ? probe.previous : -1,
                            probe.restore.publicResult, probe.restore.performResult};
    env->SetIntArrayRegion(detail, 0, 5, values);
    return static_cast<jint>(probe.set.call);
}

}  // extern "C"
