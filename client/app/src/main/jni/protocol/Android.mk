# Android.mk for rubylight-protocol, the wire formats shared with the host (protocol/ at the
# repository root). The prebuilt archives come from protocol/build-android.sh.
LOCAL_PATH := $(call my-dir)

include $(CLEAR_VARS)
LOCAL_MODULE := rubylight-protocol
LOCAL_SRC_FILES := prebuilt/$(TARGET_ARCH_ABI)/librubylight_protocol.a
LOCAL_EXPORT_C_INCLUDES := $(LOCAL_PATH)/../../../../../protocol/ffi/include
include $(PREBUILT_STATIC_LIBRARY)
