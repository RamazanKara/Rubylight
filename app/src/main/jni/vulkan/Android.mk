LOCAL_PATH := $(call my-dir)

include $(CLEAR_VARS)

LOCAL_MODULE := vulkan_renderer
LOCAL_SRC_FILES := \
    frame_pacer.cpp \
    gpu_keep_busy.cpp \
    li_compat.cpp \
    ndk_api.cpp \
    pacer_trace.cpp \
    perf_hint.cpp \
    present_scheduler.cpp \
    pyrowave_decoder.cpp \
    vk_api.cpp \
    vulkan_bridge.cpp \
    vulkan_renderer.cpp

# pyrowave_decoder.cpp includes "../pyrowave/pyrowave.h"; Nightjar keeps that header under
# pyrowave-renderer/prebuilt/include/pyrowave, so this directory makes the path resolve.
LOCAL_C_INCLUDES := $(LOCAL_PATH) $(LOCAL_PATH)/../moonlight-core/moonlight-common-c/src \
    $(LOCAL_PATH)/../pyrowave-renderer/prebuilt/include/pyrowave
LOCAL_CPPFLAGS += -std=c++17 -Wall -include $(LOCAL_PATH)/li_compat.h

LOCAL_SHARED_LIBRARIES := moonlight-core

# Vulkan and the API 26+ NDK libraries are loaded at runtime (see ndk_api.h and vk_api.h),
# since the app's native code is built for API 21
LOCAL_LDLIBS := -llog -landroid -ldl

include $(BUILD_SHARED_LIBRARY)
