# Android.mk for the PyroWave Vulkan renderer.
#
# PyroWave is built only for 64-bit ABIs (see internal/device-checklist.md). On other ABIs
# the renderer library is absent and the app never offers PyroWave.
LOCAL_PATH := $(call my-dir)

ifneq ($(filter arm64-v8a x86_64,$(TARGET_ARCH_ABI)),)

include $(CLEAR_VARS)
LOCAL_MODULE := pyrowave-shared
LOCAL_SRC_FILES := prebuilt/$(TARGET_ARCH_ABI)/libpyrowave-shared.so
LOCAL_EXPORT_C_INCLUDES := $(LOCAL_PATH)/prebuilt/include
include $(PREBUILT_SHARED_LIBRARY)

include $(CLEAR_VARS)
LOCAL_MODULE := pyrowave-renderer
LOCAL_SRC_FILES := pyrowave_renderer.cpp
LOCAL_CPPFLAGS := -std=c++17 -Wall -Wextra -Wno-missing-field-initializers -fno-exceptions -fno-rtti
LOCAL_SHARED_LIBRARIES := pyrowave-shared
LOCAL_STATIC_LIBRARIES := rubylight-protocol
ifeq ($(TARGET_ARCH_ABI),arm64-v8a)
LOCAL_C_INCLUDES += $(LOCAL_PATH)/../adrenotools/include
endif
# Vulkan is loaded with dlopen at runtime; libvulkan is not linked.
LOCAL_LDLIBS := -llog -landroid -ldl
include $(BUILD_SHARED_LIBRARY)

endif
