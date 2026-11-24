LOCAL_PATH := $(call my-dir)

include $(CLEAR_VARS)

LOCAL_MODULE := vulkan_renderer
LOCAL_SRC_FILES := \
    frame_pacer.cpp \
    frame_pacer_phase.cpp \
    frame_pacer_timeline.cpp \
    li_compat.cpp \
    ndk_api.cpp \
    pacer_trace.cpp \
    perf_hint.cpp \
    present_scheduler.cpp \
    vk_api.cpp \
    vulkan_bridge.cpp \
    vulkan_renderer.cpp \
    vulkan_renderer_frame.cpp \
    vulkan_renderer_lifecycle.cpp \
    vulkan_renderer_loop.cpp \
    vulkan_renderer_swapchain.cpp \
    vulkan_renderer_warp_import.cpp

LOCAL_C_INCLUDES := $(LOCAL_PATH)
LOCAL_CPPFLAGS += -std=c++17 -Wall -DVKR_FRAME_PACER_SEPARATE_TUS -include $(LOCAL_PATH)/li_compat.h

# Vulkan and the API 26+ NDK libraries are loaded at runtime (see ndk_api.h and vk_api.h),
# since the app's native code is built for API 25 and not every device has them
LOCAL_LDLIBS := -llog -landroid -ldl

include $(BUILD_SHARED_LIBRARY)
