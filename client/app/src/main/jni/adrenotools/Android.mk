# libadrenotools 8fae8ce254dfc1344527e05301e43f37dea2df80 (BSD-2-Clause).
# liblinkernsbypass aa3975893d83ef1bc84c321ec60c65fbf1287887 (BSD-2-Clause).
LOCAL_PATH := $(call my-dir)

ifeq ($(TARGET_ARCH_ABI),arm64-v8a)
include $(CLEAR_VARS)
LOCAL_MODULE := linkernsbypass
LOCAL_SRC_FILES := lib/linkernsbypass/android_linker_ns.cpp lib/linkernsbypass/elf_soname_patcher.cpp
LOCAL_EXPORT_C_INCLUDES := $(LOCAL_PATH)/lib/linkernsbypass
LOCAL_CPPFLAGS := -std=c++17
include $(BUILD_STATIC_LIBRARY)

include $(CLEAR_VARS)
LOCAL_MODULE := adrenotools
LOCAL_SRC_FILES := src/driver.cpp
LOCAL_C_INCLUDES := $(LOCAL_PATH)/include
LOCAL_EXPORT_C_INCLUDES := $(LOCAL_PATH)/include
LOCAL_STATIC_LIBRARIES := linkernsbypass
LOCAL_CPPFLAGS := -std=c++17
LOCAL_LDLIBS := -landroid -llog -ldl
include $(BUILD_SHARED_LIBRARY)

include $(CLEAR_VARS)
LOCAL_MODULE := hook_impl
LOCAL_SRC_FILES := src/hook/hook_impl.cpp
LOCAL_C_INCLUDES := $(LOCAL_PATH)/include
LOCAL_STATIC_LIBRARIES := linkernsbypass
LOCAL_CPPFLAGS := -std=c++17 -fvisibility=hidden
LOCAL_LDLIBS := -landroid -llog -ldl
include $(BUILD_SHARED_LIBRARY)

include $(CLEAR_VARS)
LOCAL_MODULE := main_hook
LOCAL_SRC_FILES := src/hook/main_hook.c
LOCAL_SHARED_LIBRARIES := hook_impl
LOCAL_LDFLAGS := -Wl,-z,global
include $(BUILD_SHARED_LIBRARY)
endif
