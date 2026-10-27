// Exercise the real writer with a deterministic USB backend, without opening devices.
#define USB_AUDIO_TEST
#define ioctl fake_ioctl
#define fcntl fake_fcntl
#define close fake_close
#define poll fake_poll
#include "../../main/jni/moonlight-core/dualsense_usb_audio.c"
#include <assert.h>
#include <stdarg.h>

static struct usbdevfs_urb* pending[2];
static int pending_count, closes, speed = 3;
static bool fail_submit, bad_packet, disconnected, timeout;

int fake_fcntl(int fd, int command, ...) {
    assert(fd == 10 && command == F_DUPFD_CLOEXEC);
    return 11;
}

int fake_close(int fd) {
    assert(fd == 11);
    // Kernel cancellation completes before the production writer frees these buffers.
    for (int i = 0; i < pending_count; i++) {
        assert(pending[i]->buffer != NULL);
        memset(pending[i]->buffer, 0, USB_AUDIO_BYTES);
    }
    pending_count = 0;
    closes++;
    return 0;
}

int fake_poll(struct pollfd* fds, nfds_t count, int milliseconds) {
    assert(count == 1 && fds[0].fd == 11 && milliseconds > 0 && milliseconds <= 50);
    assert(timeout);
    return 0;
}

#ifdef __ANDROID__
int fake_ioctl(int fd, int request, ...) {
#else
int fake_ioctl(int fd, unsigned long request, ...) {
#endif
    uint32_t code = (uint32_t)request;
    if (code == (uint32_t)USBDEVFS_GET_SPEED) return speed;
    assert(fd == 11);
    va_list args;
    va_start(args, request);
    void* argument = va_arg(args, void*);
    va_end(args);
    if (code == (uint32_t)USBDEVFS_SUBMITURB) {
        if (fail_submit) { errno = EIO; return -1; }
        struct usbdevfs_urb* urb = argument;
        assert(pending_count < 2);
        assert(urb->type == USBDEVFS_URB_TYPE_ISO && (urb->endpoint == 1 || urb->endpoint == 0x82));
        assert(urb->flags == USBDEVFS_URB_ISO_ASAP);
        int packet = urb->endpoint == 1 ? 384 : 196;
        assert(urb->number_of_packets == 4 && urb->buffer_length == packet * 4);
        for (int i = 0; i < 4; i++) assert(urb->iso_frame_desc[i].length == (unsigned)packet);
        pending[pending_count++] = urb;
        return 0;
    }
    assert(code == (uint32_t)USBDEVFS_REAPURBNDELAY);
    if (disconnected || timeout) {
        errno = disconnected ? ENODEV : EAGAIN;
        return -1;
    }
    assert(pending_count > 0);
    struct usbdevfs_urb* urb = pending[0];
    pending[0] = pending[--pending_count];
    urb->iso_frame_desc[0].status = bad_packet ? -EIO : 0;
    if (urb->endpoint == 0x82) {
        for (int i = 0; i < 4; i++) {
            urb->iso_frame_desc[i].actual_length = 192;
            memset((uint8_t*)urb->buffer + i * 196, i + 1, 192);
        }
    }
    *(struct usbdevfs_urb**)argument = urb;
    return 0;
}

int main(void) {
    uint8_t samples[USB_AUDIO_BYTES];
    memset(samples, 0x42, sizeof(samples));
    assert(!usb_audio_open(10, 0, 4));
    assert(!usb_audio_open(10, 1, 1));
    speed = 2;
    assert(!usb_audio_open(10, 1, 4));
    USB_AUDIO* audio = usb_audio_open(10, 1, 1);
    assert(audio);
    assert(usb_audio_write(audio, samples));
    assert(pending_count == 2);
    assert(memcmp(audio->samples[0], samples, sizeof(samples)) == 0);
    assert(usb_audio_write(audio, samples));
    assert(pending_count == 2);
    usb_audio_close(audio);
    assert(closes == 1 && pending_count == 0);
    speed = 3;
    bool* failures[] = {&fail_submit, &bad_packet, &disconnected, &timeout};
    for (int i = 0; i < 4; i++) {
        audio = usb_audio_open(10, 1, 4);
        assert(audio);
        *failures[i] = true;
        assert(!usb_audio_write(audio, samples));
        usb_audio_close(audio);
        assert(pending_count == 0);
        *failures[i] = false;
    }
    assert(closes == 5);
    assert(!usb_mic_open(10, 2, 4));
    audio = usb_mic_open(10, 0x82, 4);
    assert(audio);
    assert(usb_mic_read(audio, samples) == 768);
    for (int i = 0; i < 768; i++) assert(samples[i] == i / 192 + 1);
    bad_packet = true;
    assert(usb_mic_read(audio, samples) == -1);
    usb_audio_close(audio);
    assert(closes == 6 && pending_count == 0);
    return 0;
}
