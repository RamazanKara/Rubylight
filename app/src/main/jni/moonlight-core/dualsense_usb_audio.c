// Direct USB Audio 1.0 output for the opt-in DualSense waveform driver.
// The Java owner closes its separate UsbDeviceConnection before usb_audio_close().
#include <errno.h>
#include <fcntl.h>
#include <linux/usbdevice_fs.h>
#include <poll.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <time.h>
#include <unistd.h>

#define USB_AUDIO_PACKETS 4
#define USB_AUDIO_PACKET_BYTES (48 * 4 * 2)
#define USB_AUDIO_BYTES (USB_AUDIO_PACKETS * USB_AUDIO_PACKET_BYTES)

typedef struct {
    int fd;
    struct usbdevfs_urb* urbs[2];
    uint8_t samples[2][USB_AUDIO_BYTES];
    bool submitted[2];
} USB_AUDIO;

static void usb_audio_close(USB_AUDIO* audio) {
    if (!audio) return;
    // close() on the final reference cancels and waits for pending URBs. Buffers
    // must outlive that call, even if the controller disappeared or a poll timed out.
    close(audio->fd);
    free(audio->urbs[0]);
    free(audio->urbs[1]);
    free(audio);
}

static USB_AUDIO* usb_audio_open(int fd, int endpoint, int interval) {
    if (endpoint < 1 || endpoint > 15) return NULL;
    int speed = ioctl(fd, USBDEVFS_GET_SPEED, NULL);
    // USB_SPEED_FULL = 2, USB_SPEED_HIGH = 3. Admit only 1 ms service intervals.
    if (!((speed == 2 && interval == 1) || (speed == 3 && interval == 4))) return NULL;
    USB_AUDIO* audio = calloc(1, sizeof(*audio));
    if (!audio) return NULL;
    audio->fd = fcntl(fd, F_DUPFD_CLOEXEC, 0);
    if (audio->fd < 0) {
        free(audio);
        return NULL;
    }
    for (int i = 0; i < 2; i++) {
        struct usbdevfs_urb* urb = calloc(1, sizeof(*urb) + USB_AUDIO_PACKETS * sizeof(urb->iso_frame_desc[0]));
        audio->urbs[i] = urb;
        if (!urb) {
            usb_audio_close(audio); // No transfers have been submitted yet.
            return NULL;
        }
        urb->type = USBDEVFS_URB_TYPE_ISO;
        urb->endpoint = (unsigned char)endpoint;
        urb->flags = USBDEVFS_URB_ISO_ASAP;
        urb->buffer = audio->samples[i];
        urb->buffer_length = USB_AUDIO_BYTES;
        urb->number_of_packets = USB_AUDIO_PACKETS;
        for (int p = 0; p < USB_AUDIO_PACKETS; p++) urb->iso_frame_desc[p].length = USB_AUDIO_PACKET_BYTES;
    }
    return audio;
}

static bool usb_audio_submit(USB_AUDIO* audio, int index) {
    struct usbdevfs_urb* urb = audio->urbs[index];
    urb->status = urb->error_count = urb->actual_length = 0;
    for (int p = 0; p < USB_AUDIO_PACKETS; p++) {
        urb->iso_frame_desc[p].status = urb->iso_frame_desc[p].actual_length = 0;
    }
    if (ioctl(audio->fd, USBDEVFS_SUBMITURB, urb) < 0) return false;
    audio->submitted[index] = true;
    return true;
}

static int64_t usb_audio_millis(void) {
    struct timespec now;
    clock_gettime(CLOCK_MONOTONIC, &now);
    return (int64_t)now.tv_sec * 1000 + now.tv_nsec / 1000000;
}

static bool usb_audio_write(USB_AUDIO* audio, const uint8_t* samples) {
    if (!audio || !samples) return false;
    // Prime with silence, then replace completed transfers. Even the initial call
    // must observe a successful completion before Java reports waveform support.
    for (int i = 0; i < 2; i++) {
        if (!audio->submitted[i] && !usb_audio_submit(audio, i)) return false;
    }
    int64_t deadline = usb_audio_millis() + 50;
    for (;;) {
        struct usbdevfs_urb* completed = NULL;
        if (ioctl(audio->fd, USBDEVFS_REAPURBNDELAY, &completed) == 0) {
            int index = completed == audio->urbs[0] ? 0 : completed == audio->urbs[1] ? 1 : -1;
            if (index < 0) return false; // This fd belongs exclusively to this audio writer.
            audio->submitted[index] = false;
            if (completed->status || completed->error_count) return false;
            for (int p = 0; p < USB_AUDIO_PACKETS; p++) {
                if (completed->iso_frame_desc[p].status) return false;
            }
            memcpy(audio->samples[index], samples, USB_AUDIO_BYTES);
            return usb_audio_submit(audio, index);
        }
        if (errno != EAGAIN && errno != EINTR) return false;
        int timeout = (int)(deadline - usb_audio_millis());
        if (timeout <= 0) return false;
        struct pollfd wait = {.fd = audio->fd, .events = POLLOUT};
        int result = poll(&wait, 1, timeout);
        if ((result < 0 && errno != EINTR) || result == 0 ||
                (result > 0 && (wait.revents & (POLLERR | POLLHUP | POLLNVAL)))) return false;
    }
}

// UAC1 microphone: up to 49 stereo frames in each asynchronous 1 ms packet.
static USB_AUDIO* usb_mic_open(int fd, int endpoint, int interval) {
    if ((endpoint & 0xf0) != 0x80 || !(endpoint & 15)) return NULL;
    USB_AUDIO* audio = usb_audio_open(fd, endpoint & 15, interval);
    if (!audio) return NULL;
    for (int i = 0; i < 2; i++) {
        audio->urbs[i]->endpoint = (unsigned char)endpoint;
        audio->urbs[i]->buffer_length = 4 * 196;
        for (int p = 0; p < 4; p++) audio->urbs[i]->iso_frame_desc[p].length = 196;
    }
    return audio;
}
static int usb_mic_read(USB_AUDIO* audio, uint8_t* samples) {
    if (!audio || !samples) return -1;
    for (int i = 0; i < 2; i++) if (!audio->submitted[i] && !usb_audio_submit(audio, i)) return -1;
    int64_t deadline = usb_audio_millis() + 50;
    for (;;) {
        struct usbdevfs_urb* completed = NULL;
        if (ioctl(audio->fd, USBDEVFS_REAPURBNDELAY, &completed) == 0) {
            int index = completed == audio->urbs[0] ? 0 : completed == audio->urbs[1] ? 1 : -1;
            if (index < 0) return -1;
            audio->submitted[index] = false;
            if (completed->status || completed->error_count) return -1;
            int count = 0;
            for (int p = 0; p < 4; p++) {
                unsigned length = completed->iso_frame_desc[p].actual_length;
                if (completed->iso_frame_desc[p].status || length > 196 || length % 4) return -1;
                memcpy(samples + count, audio->samples[index] + p * 196, length);
                count += (int)length;
            }
            return usb_audio_submit(audio, index) ? count : -1;
        }
        if (errno != EAGAIN && errno != EINTR) return -1;
        int timeout = (int)(deadline - usb_audio_millis());
        if (timeout <= 0) return -1;
        struct pollfd wait = {.fd = audio->fd, .events = POLLOUT};
        int result = poll(&wait, 1, timeout);
        if ((result < 0 && errno != EINTR) || result == 0 || (result > 0 && (wait.revents & (POLLERR | POLLHUP | POLLNVAL)))) return -1;
    }
}

#ifndef USB_AUDIO_TEST
#include <jni.h>

JNIEXPORT jlong JNICALL
Java_com_limelight_nvstream_jni_MoonBridge_openUsbHaptics(JNIEnv* env, jclass clazz,
                                                       jint fd, jint endpoint, jint interval) {
    (void)env; (void)clazz;
    return (jlong)(intptr_t)usb_audio_open(fd, endpoint, interval);
}

JNIEXPORT jboolean JNICALL
Java_com_limelight_nvstream_jni_MoonBridge_writeUsbHaptics(JNIEnv* env, jclass clazz,
                                                        jlong handle, jbyteArray samples) {
    (void)clazz;
    if (!handle || !samples || (*env)->GetArrayLength(env, samples) != USB_AUDIO_BYTES) return JNI_FALSE;
    uint8_t buffer[USB_AUDIO_BYTES];
    (*env)->GetByteArrayRegion(env, samples, 0, sizeof(buffer), (jbyte*)buffer);
    if ((*env)->ExceptionCheck(env)) return JNI_FALSE;
    return usb_audio_write((USB_AUDIO*)(intptr_t)handle, buffer) ? JNI_TRUE : JNI_FALSE;
}

JNIEXPORT void JNICALL
Java_com_limelight_nvstream_jni_MoonBridge_closeUsbHaptics(JNIEnv* env, jclass clazz, jlong handle) {
    (void)env; (void)clazz;
    usb_audio_close((USB_AUDIO*)(intptr_t)handle);
}

JNIEXPORT jlong JNICALL
Java_com_limelight_nvstream_jni_MoonBridge_openUsbMicrophone(JNIEnv* env, jclass clazz, jint fd, jint endpoint, jint interval) {
    (void)env; (void)clazz;
    return (jlong)(intptr_t)usb_mic_open(fd, endpoint, interval);
}
JNIEXPORT jint JNICALL
Java_com_limelight_nvstream_jni_MoonBridge_readUsbMicrophone(JNIEnv* env, jclass clazz, jlong handle, jbyteArray pcm) {
    (void)clazz;
    if (!handle || !pcm || (*env)->GetArrayLength(env, pcm) < 784) return -1;
    uint8_t samples[784];
    int count = usb_mic_read((USB_AUDIO*)(intptr_t)handle, samples);
    if (count > 0) (*env)->SetByteArrayRegion(env, pcm, 0, count, (jbyte*)samples);
    return (*env)->ExceptionCheck(env) ? -1 : count;
}

JNIEXPORT void JNICALL
Java_com_limelight_nvstream_jni_MoonBridge_restoreUsbAudio(JNIEnv* env, jclass clazz,
                                                        jint fd, jint controlInterface) {
    (void)env; (void)clazz;
    struct usbdevfs_ioctl request = {.ifno = controlInterface, .ioctl_code = USBDEVFS_CONNECT};
    ioctl(fd, USBDEVFS_IOCTL, &request);
}
#endif
