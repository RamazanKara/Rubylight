NIGHTJAR_JNI_PATH := $(call my-dir)
NIGHTJAR_JNI_MAKEFILES := $(call all-subdir-makefiles)
include $(NIGHTJAR_JNI_PATH)/adrenotools/Android.mk
include $(filter-out $(NIGHTJAR_JNI_PATH)/adrenotools/Android.mk,$(NIGHTJAR_JNI_MAKEFILES))
