# Android.mk for moonlight-core and binding
MY_LOCAL_PATH := $(call my-dir)

include $(call all-subdir-makefiles)

LOCAL_PATH := $(MY_LOCAL_PATH)

include $(CLEAR_VARS)
LOCAL_MODULE    := moonlight-core

LOCAL_SRC_FILES := moonlight-common-c/src/AudioStream.c \
                   moonlight-common-c/src/ByteBuffer.c \
                   android_connection.c \
                   moonlight-common-c/src/ConnectionTester.c \
                   android_control.c \
                   android_present_clock.c \
                   moonlight-common-c/src/FakeCallbacks.c \
                   android_input.c \
                   dualsense_usb_audio.c \
                   aaudio_output.c \
                   android_microphone.c \
                   latency.c \
                   moonlight-common-c/src/LinkedBlockingQueue.c \
                   moonlight-common-c/src/Misc.c \
                   moonlight-common-c/src/Platform.c \
                   moonlight-common-c/src/PlatformCrypto.c \
                   android_sockets.c \
                   moonlight-common-c/src/RtpAudioQueue.c \
                   android_rtp_video.c \
                   android_rtsp.c \
                   android_rtsp_parser.c \
                   android_sdp.c \
                   android_stun.c \
                   android_depacketizer.c \
                   moonlight-common-c/src/VideoStream.c \
                   moonlight-common-c/nanors/deps/obl/oblas_common.c \
                   moonlight-common-c/nanors/deps/obl/oblas_lite.c \
                   moonlight-common-c/nanors/rs.c \
                   moonlight-common-c/enet/callbacks.c \
                   moonlight-common-c/enet/compress.c \
                   moonlight-common-c/enet/host.c \
                   moonlight-common-c/enet/list.c \
                   moonlight-common-c/enet/packet.c \
                   moonlight-common-c/enet/peer.c \
                   moonlight-common-c/enet/protocol.c \
                   moonlight-common-c/enet/unix.c \
                   moonlight-common-c/enet/win32.c \
                   simplejni.c \
                   callbacks.c \
                   minisdl.c \


LOCAL_C_INCLUDES := $(LOCAL_PATH) \
                    $(LOCAL_PATH)/moonlight-common-c/enet/include \
                    $(LOCAL_PATH)/moonlight-common-c/nanors/deps/obl \
                    $(LOCAL_PATH)/moonlight-common-c/nanors \
                    $(LOCAL_PATH)/moonlight-common-c/src \

LOCAL_CFLAGS := -DHAS_SOCKLEN_T=1 -DLC_ANDROID -DHAVE_CLOCK_GETTIME=1

ifeq ($(NDK_DEBUG),1)
LOCAL_CFLAGS += -DLC_DEBUG
endif

LOCAL_LDLIBS := -llog -ldl

LOCAL_STATIC_LIBRARIES := libopus libcrypto cpufeatures rubylight-protocol
LOCAL_LDFLAGS += -Wl,--exclude-libs,ALL
LOCAL_LDFLAGS += -Wl,--wrap=socket -Wl,--wrap=pthread_setname_np

LOCAL_BRANCH_PROTECTION := standard

include $(BUILD_SHARED_LIBRARY)

$(call import-module,android/cpufeatures)
