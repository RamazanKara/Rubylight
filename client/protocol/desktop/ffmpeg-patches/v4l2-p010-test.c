// A fake stateful V4L2 decoder, inside this program, for FFmpeg's hevc_v4l2m2m: it answers the system
// calls the way the Iris driver does (Linux 7.2 and later). On a 10-bit stream the capture queue
// moves to P010 at the source change, at the same size, and NV12 buffers are refused from then on.
//
//   v4l2-p010-test 8|10
//
// Exits 0 when the first picture has the format and plane layout of that bit depth. Built by
// test-v4l2-p010.sh, which has no need for a device.
#define _GNU_SOURCE
#include <dirent.h>
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <linux/videodev2.h>
#include <poll.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <unistd.h>

#include <libavcodec/avcodec.h>
#include <libavutil/frame.h>
#include <libavutil/pixdesc.h>

#ifndef V4L2_PIX_FMT_P010
#define V4L2_PIX_FMT_P010 v4l2_fourcc('P', '0', '1', '0')
#endif

#define FAKE_FD 999
#define MAX_BUFFERS 64
#define WIDTH 64
#define HEIGHT 32

static int ten_bit_stream;
static int stream_seen, event_pending, capture_on, output_on, input_count;
static uint32_t capture_format = V4L2_PIX_FMT_NV12;
static int capture_queued[MAX_BUFFERS], output_done[MAX_BUFFERS];
static uint8_t *maps[2][MAX_BUFFERS];

static uint32_t stride(uint32_t format) { return format == V4L2_PIX_FMT_P010 ? WIDTH * 2 : WIDTH; }
static uint32_t size_of(uint32_t format) { return stride(format) * HEIGHT * 3 / 2; }

static void fail(const char *why)
{
    fprintf(stderr, "v4l2-p010-test: %s\n", why);
    exit(1);
}

static void fill_format(struct v4l2_format *f)
{
    struct v4l2_pix_format_mplane *mp = &f->fmt.pix_mp;
    mp->width = WIDTH;
    mp->height = HEIGHT;
    mp->num_planes = 1;
    if (f->type == V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE) {
        if (mp->pixelformat != V4L2_PIX_FMT_P010 && mp->pixelformat != V4L2_PIX_FMT_NV12)
            mp->pixelformat = V4L2_PIX_FMT_NV12;
        // The driver moves a 10-bit stream's queue: NV12 cannot hold it.
        if (ten_bit_stream && stream_seen)
            mp->pixelformat = V4L2_PIX_FMT_P010;
        mp->plane_fmt[0].bytesperline = stride(mp->pixelformat);
        mp->plane_fmt[0].sizeimage = size_of(mp->pixelformat);
    } else {
        mp->pixelformat = V4L2_PIX_FMT_HEVC;
        mp->plane_fmt[0].bytesperline = 0;
        mp->plane_fmt[0].sizeimage = 1 << 16;
    }
}

static int capture_ready(void)
{
    if (!capture_on || !input_count || event_pending)
        return -1;
    for (int i = 0; i < MAX_BUFFERS; i++)
        if (capture_queued[i])
            return i;
    return -1;
}

static int output_ready(void)
{
    for (int i = 0; i < MAX_BUFFERS; i++)
        if (output_done[i])
            return i;
    return -1;
}

static int fake_ioctl(unsigned long request, void *arg)
{
    switch (request) {
    case VIDIOC_QUERYCAP: {
        struct v4l2_capability *cap = arg;
        memset(cap, 0, sizeof(*cap));
        strcpy((char *)cap->driver, "fake-iris");
        cap->device_caps = V4L2_CAP_VIDEO_M2M_MPLANE | V4L2_CAP_STREAMING;
        cap->capabilities = cap->device_caps | V4L2_CAP_DEVICE_CAPS;
        return 0;
    }
    case VIDIOC_ENUM_FMT: {
        struct v4l2_fmtdesc *d = arg;
        if (d->type == V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE && d->index == 0) {
            d->pixelformat = V4L2_PIX_FMT_HEVC;
            return 0;
        }
        if (d->type == V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE && d->index < 2) {
            d->pixelformat = d->index ? V4L2_PIX_FMT_P010 : V4L2_PIX_FMT_NV12;
            return 0;
        }
        errno = EINVAL;
        return -1;
    }
    case VIDIOC_G_FMT: {
        struct v4l2_format *f = arg;
        f->fmt.pix_mp.pixelformat = capture_format;
        fill_format(f);
        return 0;
    }
    case VIDIOC_TRY_FMT:
        fill_format(arg);
        return 0;
    case VIDIOC_S_FMT: {
        struct v4l2_format *f = arg;
        fill_format(f);
        if (f->type == V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE)
            capture_format = f->fmt.pix_mp.pixelformat;
        return 0;
    }
    case VIDIOC_REQBUFS: {
        struct v4l2_requestbuffers *r = arg;
        if (r->count > MAX_BUFFERS)
            r->count = MAX_BUFFERS;
        if (r->type == V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE)
            memset(capture_queued, 0, sizeof(capture_queued));
        return 0;
    }
    case VIDIOC_QUERYBUF: {
        struct v4l2_buffer *b = arg;
        int capture = b->type == V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
        b->length = 1;
        b->m.planes[0].length = capture ? size_of(capture_format) : 1 << 16;
        b->m.planes[0].m.mem_offset = (uint32_t)capture << 16 | b->index;
        return 0;
    }
    case VIDIOC_QBUF: {
        struct v4l2_buffer *b = arg;
        if (b->type == V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE) {
            output_done[b->index] = 1;
            input_count++;
            if (!stream_seen) {
                stream_seen = 1;
                event_pending = 1;
                if (ten_bit_stream)
                    capture_format = V4L2_PIX_FMT_P010;
            }
        } else {
            capture_queued[b->index] = 1;
        }
        return 0;
    }
    case VIDIOC_DQBUF: {
        struct v4l2_buffer *b = arg;
        int index;
        if (b->type == V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE) {
            if ((index = output_ready()) < 0)
                break;
            output_done[index] = 0;
            b->index = index;
            b->length = 1;
            b->m.planes[0].bytesused = 1;
            return 0;
        }
        if ((index = capture_ready()) < 0)
            break;
        capture_queued[index] = 0;
        b->index = index;
        b->length = 1;
        b->m.planes[0].bytesused = size_of(capture_format);
        b->flags = 0;
        // Luma and chroma that tell the planes apart, in the capture format's sample size.
        uint8_t *map = maps[1][index];
        memset(map, 0x11, stride(capture_format) * HEIGHT);
        memset(map + stride(capture_format) * HEIGHT, 0x77, stride(capture_format) * HEIGHT / 2);
        return 0;
    }
    case VIDIOC_DQEVENT: {
        struct v4l2_event *e = arg;
        if (!event_pending)
            break;
        memset(e, 0, sizeof(*e));
        e->type = V4L2_EVENT_SOURCE_CHANGE;
        e->u.src_change.changes = V4L2_EVENT_SRC_CH_RESOLUTION;
        event_pending = 0;
        return 0;
    }
    case VIDIOC_STREAMON:
    case VIDIOC_STREAMOFF: {
        int on = request == VIDIOC_STREAMON;
        if (*(int *)arg == V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE) {
            capture_on = on;
            if (!on)
                memset(capture_queued, 0, sizeof(capture_queued));
        } else {
            output_on = on;
        }
        return 0;
    }
    case VIDIOC_SUBSCRIBE_EVENT:
    case VIDIOC_DECODER_CMD:
        return 0;
    default:
        errno = ENOTTY;
        return -1;
    }
    errno = EAGAIN;
    return -1;
}

// The system calls FFmpeg makes, here instead of in the C library (linked with --export-dynamic so
// libavcodec binds to them), for the one device this program pretends /dev has.
int ioctl(int fd, unsigned long request, ...)
{
    va_list args;
    va_start(args, request);
    void *arg = va_arg(args, void *);
    va_end(args);
    if (fd == FAKE_FD)
        return fake_ioctl(request, arg);
    return (int)syscall(SYS_ioctl, fd, request, arg);
}

static DIR *const fake_dir = (DIR *)&capture_format;
static int dir_read;

DIR *opendir(const char *name)
{
    (void)name;
    dir_read = 0;
    return fake_dir;
}

int closedir(DIR *dir)
{
    (void)dir;
    return 0;
}

struct dirent *readdir(DIR *dir)
{
    static struct dirent entry;
    (void)dir;
    if (dir_read++)
        return NULL;
    strcpy(entry.d_name, "video99");
    return &entry;
}

struct dirent64 *readdir64(DIR *dir)
{
    return (struct dirent64 *)readdir(dir);
}

static int open_fake(const char *path, const char *symbol, int flags, mode_t mode)
{
    if (!strcmp(path, "/dev/video99"))
        return FAKE_FD;
    int (*real)(const char *, int, ...) = dlsym(RTLD_NEXT, symbol);
    return real(path, flags, mode);
}

int open(const char *path, int flags, ...)
{
    va_list args;
    va_start(args, flags);
    mode_t mode = va_arg(args, mode_t);
    va_end(args);
    return open_fake(path, "open", flags, mode);
}

int open64(const char *path, int flags, ...)
{
    va_list args;
    va_start(args, flags);
    mode_t mode = va_arg(args, mode_t);
    va_end(args);
    return open_fake(path, "open64", flags, mode);
}

int close(int fd)
{
    if (fd == FAKE_FD)
        return 0;
    return (int)syscall(SYS_close, fd);
}

void *mmap(void *addr, size_t length, int prot, int flags, int fd, off_t offset)
{
    if (fd == FAKE_FD) {
        int capture = (int)(offset >> 16), index = (int)(offset & 0xffff);
        uint8_t *map = calloc(1, length);
        if (!map)
            return MAP_FAILED;
        maps[capture][index] = map;
        return map;
    }
    return (void *)syscall(SYS_mmap, addr, length, prot, flags, fd, offset);
}

void *mmap64(void *addr, size_t length, int prot, int flags, int fd, off_t offset)
{
    return mmap(addr, length, prot, flags, fd, offset);
}

int munmap(void *addr, size_t length)
{
    for (int q = 0; q < 2; q++)
        for (int i = 0; i < MAX_BUFFERS; i++)
            if (maps[q][i] == addr) {
                free(addr);
                maps[q][i] = NULL;
                return 0;
            }
    return (int)syscall(SYS_munmap, addr, length);
}

int poll(struct pollfd *fds, nfds_t count, int timeout)
{
    (void)count;
    (void)timeout;
    short events = fds->events, revents = 0;
    if ((events & POLLPRI) && event_pending)
        revents |= POLLPRI;
    if ((events & (POLLIN | POLLRDNORM)) && capture_ready() >= 0)
        revents |= POLLIN;
    if ((events & (POLLOUT | POLLWRNORM)) && output_ready() >= 0)
        revents |= POLLOUT;
    fds->revents = revents;
    if (!revents)
        errno = ETIMEDOUT;
    return revents ? 1 : 0;
}

int main(int argc, char **argv)
{
    if (argc != 2 || (strcmp(argv[1], "8") && strcmp(argv[1], "10")))
        fail("usage: v4l2-p010-test 8|10");
    ten_bit_stream = !strcmp(argv[1], "10");

    const AVCodec *codec = avcodec_find_decoder_by_name("hevc_v4l2m2m");
    if (!codec)
        fail("this FFmpeg has no hevc_v4l2m2m");
    AVCodecContext *context = avcodec_alloc_context3(codec);
    context->coded_width = context->width = WIDTH;
    context->coded_height = context->height = HEIGHT;
    int ret = avcodec_open2(context, codec, NULL);
    if (ret < 0)
        fail("avcodec_open2 failed");

    static uint8_t bytes[64] = { 0, 0, 0, 1, 0x26, 0x01, 0xaf };
    AVPacket *packet = av_packet_alloc();
    AVFrame *frame = av_frame_alloc();
    for (int attempt = 0; attempt < 200; attempt++) {
        av_new_packet(packet, sizeof(bytes));
        memcpy(packet->data, bytes, sizeof(bytes));
        ret = avcodec_send_packet(context, packet);
        av_packet_unref(packet);
        if (ret < 0 && ret != AVERROR(EAGAIN))
            fail("avcodec_send_packet failed");
        ret = avcodec_receive_frame(context, frame);
        if (ret == AVERROR(EAGAIN))
            continue;
        if (ret < 0)
            fail("avcodec_receive_frame failed");

        enum AVPixelFormat want = ten_bit_stream ? AV_PIX_FMT_P010LE : AV_PIX_FMT_NV12;
        int line = ten_bit_stream ? WIDTH * 2 : WIDTH;
        if (frame->format != want)
            fail(ten_bit_stream ? "the picture is not P010" : "the picture is not NV12");
        if (context->pix_fmt != want)
            fail("the codec context does not say the picture's format");
        if (frame->linesize[0] != line || frame->linesize[1] != line)
            fail("wrong line sizes");
        if (frame->data[1] != frame->data[0] + line * HEIGHT)
            fail("the chroma plane is not after the luma plane");
        if (frame->data[0][0] != 0x11 || frame->data[1][0] != 0x77)
            fail("the planes do not hold the decoder's samples");
        printf("v4l2-p010-test: %s-bit stream: %s, %dx%d, line %d\n", argv[1], av_get_pix_fmt_name(frame->format), frame->width, frame->height, line);
        av_frame_free(&frame);
        av_packet_free(&packet);
        avcodec_free_context(&context);
        return 0;
    }
    fail("no picture after 200 packets");
    return 1;
}
