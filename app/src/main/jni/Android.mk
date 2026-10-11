RUBYLIGHT_JNI_PATH := $(call my-dir)
RUBYLIGHT_JNI_MAKEFILES := $(call all-subdir-makefiles)
include $(RUBYLIGHT_JNI_PATH)/adrenotools/Android.mk
include $(filter-out $(RUBYLIGHT_JNI_PATH)/adrenotools/Android.mk,$(RUBYLIGHT_JNI_MAKEFILES))
