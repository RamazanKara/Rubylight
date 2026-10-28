# Android.mk for the window dataspace probe (WindowDataspace.java), on every ABI: PyroWave asks it
# now, and the Vulkan renderer can too.
LOCAL_PATH := $(call my-dir)

include $(CLEAR_VARS)
LOCAL_MODULE := window-dataspace
LOCAL_SRC_FILES := window_dataspace_jni.cpp
LOCAL_CPPFLAGS := -std=c++17 -Wall -Wextra -fno-exceptions -fno-rtti
# The API 28 dataspace calls are looked up at run time.
LOCAL_LDLIBS := -landroid -ldl
include $(BUILD_SHARED_LIBRARY)
