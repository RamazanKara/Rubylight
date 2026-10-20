/* Nightjar desktop: a narrow wrapper over libavcodec and libopus, so Rust never touches
 * FFmpeg structs.
 *
 * Copyright (C) 2026 RamazanKara. GPL-3.0-only.
 *
 * The decoder setup (low delay, slice threads, hardware device sharing, D3D11 surface copy
 * under the device lock) follows moonlight-qt's ffmpeg.cpp and d3d11va.cpp (GPL-3.0, Cameron
 * Gutman and contributors) and nightjar-cli's shim. */
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <libavcodec/avcodec.h>
#include <libavutil/hwcontext.h>
#include <libavutil/pixdesc.h>
#include <opus_multistream.h>

#ifdef _WIN32
#define COBJMACROS
#include <d3d11.h>
#include <d3d10.h>
#include <dxgi1_2.h>
#include <libavutil/hwcontext_d3d11va.h>
/* Defined here so nothing links dxguid. */
static const GUID nj_IID_ID3D10Multithread = {0x9b7e4e00, 0x342c, 0x4106, {0xa1, 0x9f, 0x4f, 0x27, 0x04, 0xf6, 0x89, 0xf0}};
#endif

enum { NJ_HW_NONE = 0, NJ_HW_D3D11 = 1, NJ_HW_VAAPI = 2 };

/* Frame kinds the renderer understands. */
enum {
    NJ_FRAME_NV12 = 0,
    NJ_FRAME_P010 = 1,
    NJ_FRAME_YUV420P = 2,
    NJ_FRAME_D3D11 = 3,
    NJ_FRAME_OTHER = 4,
};

typedef struct nj_video {
    AVCodecContext *context;
    AVPacket *packet;
    enum AVPixelFormat hardware_format;
    int hardware;
} nj_video;

typedef struct nj_frame_info {
    int width, height, kind, ten_bit;
    int64_t pts;
    uint8_t *data[3];
    int linesize[3];
    void *texture;
    int texture_index;
    int color_space, color_range, color_trc, color_primaries;
} nj_frame_info;

static enum AVPixelFormat select_format(AVCodecContext *context, const enum AVPixelFormat *formats) {
    nj_video *video = context->opaque;
    for (const enum AVPixelFormat *format = formats; *format != AV_PIX_FMT_NONE; format++)
        if (*format == video->hardware_format) return *format;
    /* The hardware path is gone (unsupported profile): take the first software format. */
    for (const enum AVPixelFormat *format = formats; *format != AV_PIX_FMT_NONE; format++) {
        const AVPixFmtDescriptor *desc = av_pix_fmt_desc_get(*format);
        if (desc && !(desc->flags & AV_PIX_FMT_FLAG_HWACCEL)) return *format;
    }
    return AV_PIX_FMT_NONE;
}

void nj_video_close(nj_video *video) {
    if (!video) return;
    avcodec_free_context(&video->context);
    av_packet_free(&video->packet);
    av_free(video);
}

static int find_hw_format(const AVCodec *codec, enum AVHWDeviceType type, enum AVPixelFormat *out) {
    for (int i = 0;; i++) {
        const AVCodecHWConfig *config = avcodec_get_hw_config(codec, i);
        if (!config) return 0;
        if (config->device_type == type && (config->methods & AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX)) {
            *out = config->pix_fmt;
            return 1;
        }
    }
}

/* codec: 0 H.264, 1 HEVC, 2 AV1. hardware: NJ_HW_*; for D3D11 `device` is the renderer's
 * ID3D11Device, shared so decoded surfaces can be copied on the GPU. Writes a description
 * ("h264 d3d11va", "libdav1d") to `name`. NULL with `error` filled on failure. */
nj_video *nj_video_open(int codec, int hardware, void *device, int threads, char *name, int name_length,
                        char *error, int error_length) {
    enum AVCodecID id = codec == 0 ? AV_CODEC_ID_H264 : codec == 1 ? AV_CODEC_ID_HEVC : AV_CODEC_ID_AV1;
    /* Hardware AV1 runs through FFmpeg's native av1 decoder; libdav1d has no hwaccel. */
    const AVCodec *found = (hardware && id == AV_CODEC_ID_AV1) ? avcodec_find_decoder_by_name("av1") : NULL;
    if (!found) found = avcodec_find_decoder(id);
    error[0] = 0;
    name[0] = 0;
    if (!found) {
        snprintf(error, error_length, "no decoder for this codec");
        return NULL;
    }
    nj_video *video = av_mallocz(sizeof(*video));
    if (!video) return NULL;
    video->hardware_format = AV_PIX_FMT_NONE;
    video->context = avcodec_alloc_context3(found);
    video->packet = av_packet_alloc();
    if (!video->context || !video->packet) goto fail;
    video->context->opaque = video;
    if (hardware == NJ_HW_D3D11) {
#ifdef _WIN32
        if (!find_hw_format(found, AV_HWDEVICE_TYPE_D3D11VA, &video->hardware_format)) {
            snprintf(error, error_length, "%s has no D3D11VA support", found->name);
            goto fail;
        }
        AVBufferRef *ref = av_hwdevice_ctx_alloc(AV_HWDEVICE_TYPE_D3D11VA);
        if (!ref) goto fail;
        AVD3D11VADeviceContext *d3d = ((AVHWDeviceContext *)ref->data)->hwctx;
        d3d->device = (ID3D11Device *)device;
        ID3D11Device_AddRef(d3d->device);
        /* The renderer draws on the immediate context while the decoder thread uses it:
         * serialise every call (FFmpeg does this only for devices it creates itself). */
        ID3D10Multithread *multithread = NULL;
        if (SUCCEEDED(ID3D11Device_QueryInterface(d3d->device, &nj_IID_ID3D10Multithread, (void **)&multithread))) {
            ID3D10Multithread_SetMultithreadProtected(multithread, TRUE);
            ID3D10Multithread_Release(multithread);
        }
        int result = av_hwdevice_ctx_init(ref);
        if (result < 0) {
            av_buffer_unref(&ref);
            snprintf(error, error_length, "D3D11VA device init failed (%d)", result);
            goto fail;
        }
        video->context->hw_device_ctx = ref;
#else
        (void)device;
        snprintf(error, error_length, "D3D11VA is Windows only");
        goto fail;
#endif
    } else if (hardware == NJ_HW_VAAPI) {
        if (!find_hw_format(found, AV_HWDEVICE_TYPE_VAAPI, &video->hardware_format)) {
            snprintf(error, error_length, "%s has no VAAPI support", found->name);
            goto fail;
        }
        int result = av_hwdevice_ctx_create(&video->context->hw_device_ctx, AV_HWDEVICE_TYPE_VAAPI, NULL, NULL, 0);
        if (result < 0) {
            snprintf(error, error_length, "no VAAPI device (%d)", result);
            goto fail;
        }
    }
    video->hardware = hardware;
    if (hardware) {
        video->context->get_format = select_format;
        /* Surfaces held by the presenter's queue and the texture copy. */
        video->context->extra_hw_frames = 6;
    }
    video->context->thread_count = hardware ? 1 : threads;
    video->context->thread_type = FF_THREAD_SLICE;
    video->context->flags |= AV_CODEC_FLAG_LOW_DELAY;
    video->context->flags2 |= AV_CODEC_FLAG2_FAST;
    AVDictionary *options = NULL;
    if (strcmp(found->name, "libdav1d") == 0) av_dict_set(&options, "max_frame_delay", "1", 0);
    int result = avcodec_open2(video->context, found, &options);
    av_dict_free(&options);
    if (result < 0) {
        char text[128];
        av_strerror(result, text, sizeof(text));
        snprintf(error, error_length, "avcodec_open2: %s", text);
        goto fail;
    }
    snprintf(name, name_length, "%s%s", found->name,
             hardware == NJ_HW_D3D11 ? " d3d11va" : hardware == NJ_HW_VAAPI ? " vaapi" : "");
    return video;
fail:
    nj_video_close(video);
    return NULL;
}

/* avcodec_send_packet's result. */
int nj_video_send(nj_video *video, const uint8_t *data, int length, int64_t pts) {
    av_packet_unref(video->packet);
    if (av_new_packet(video->packet, length) < 0) return -1;
    memcpy(video->packet->data, data, length);
    video->packet->pts = pts;
    return avcodec_send_packet(video->context, video->packet);
}

/* The next decoded picture, or NULL with *status 1 (none ready) or negative (an error).
 * VAAPI pictures are read back to NV12/P010 here, on the decoder thread. */
AVFrame *nj_video_receive(nj_video *video, int *status) {
    AVFrame *frame = av_frame_alloc();
    if (!frame) {
        *status = -1;
        return NULL;
    }
    int result = avcodec_receive_frame(video->context, frame);
    if (result == AVERROR(EAGAIN) || result == AVERROR_EOF) {
        av_frame_free(&frame);
        *status = 1;
        return NULL;
    }
    if (result < 0) {
        av_frame_free(&frame);
        *status = result;
        return NULL;
    }
    if (frame->format == AV_PIX_FMT_VAAPI) {
        AVFrame *memory = av_frame_alloc();
        if (!memory || av_hwframe_transfer_data(memory, frame, 0) < 0 || av_frame_copy_props(memory, frame) < 0) {
            av_frame_free(&memory);
            av_frame_free(&frame);
            *status = -2;
            return NULL;
        }
        av_frame_free(&frame);
        frame = memory;
    }
    *status = 0;
    return frame;
}

void nj_frame_free(AVFrame *frame) { av_frame_free(&frame); }

void nj_frame_describe(const AVFrame *frame, nj_frame_info *info) {
    memset(info, 0, sizeof(*info));
    info->width = frame->width;
    info->height = frame->height;
    info->pts = frame->pts;
    info->color_space = frame->colorspace;
    info->color_range = frame->color_range;
    info->color_trc = frame->color_trc;
    info->color_primaries = frame->color_primaries;
    for (int i = 0; i < 3; i++) {
        info->data[i] = frame->data[i];
        info->linesize[i] = frame->linesize[i];
    }
    switch (frame->format) {
    case AV_PIX_FMT_NV12: info->kind = NJ_FRAME_NV12; break;
    case AV_PIX_FMT_P010LE: info->kind = NJ_FRAME_P010; info->ten_bit = 1; break;
    case AV_PIX_FMT_YUV420P:
    case AV_PIX_FMT_YUVJ420P: info->kind = NJ_FRAME_YUV420P; break;
#ifdef _WIN32
    case AV_PIX_FMT_D3D11: {
        info->kind = NJ_FRAME_D3D11;
        info->texture = frame->data[0];
        info->texture_index = (int)(intptr_t)frame->data[1];
        if (frame->hw_frames_ctx) {
            AVHWFramesContext *frames = (AVHWFramesContext *)frame->hw_frames_ctx->data;
            info->ten_bit = frames->sw_format == AV_PIX_FMT_P010LE;
        }
        break;
    }
#endif
    default: info->kind = NJ_FRAME_OTHER; break;
    }
}

const char *nj_pixel_format_name(const AVFrame *frame) {
    const char *name = av_get_pix_fmt_name(frame->format);
    return name ? name : "unknown";
}

/* Copies a D3D11 decoder surface into `destination` (an NV12/P010 texture of the renderer's
 * device) under FFmpeg's device lock. 0 on success. */
int nj_frame_copy_d3d11(const AVFrame *frame, void *destination, int width, int height) {
#ifdef _WIN32
    if (frame->format != AV_PIX_FMT_D3D11 || !frame->hw_frames_ctx) return -1;
    AVHWFramesContext *frames = (AVHWFramesContext *)frame->hw_frames_ctx->data;
    AVD3D11VADeviceContext *d3d = frames->device_ctx->hwctx;
    D3D11_BOX box = {0, 0, 0, (UINT)(width & ~1), (UINT)(height & ~1), 1};
    d3d->lock(d3d->lock_ctx);
    ID3D11DeviceContext_CopySubresourceRegion(d3d->device_context, (ID3D11Resource *)destination, 0, 0, 0, 0,
                                              (ID3D11Resource *)frame->data[0], (UINT)(intptr_t)frame->data[1],
                                              &box);
    d3d->unlock(d3d->lock_ctx);
    return 0;
#else
    (void)frame; (void)destination; (void)width; (void)height;
    return -1;
#endif
}

/* Whether a DXGI swap chain was created with DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING, which
 * Present needs for variable refresh in a window. */
int nj_swapchain_allows_tearing(void *swapchain) {
#ifdef _WIN32
    if (!swapchain) return 0;
    DXGI_SWAP_CHAIN_DESC1 desc;
    if (FAILED(IDXGISwapChain1_GetDesc1((IDXGISwapChain1 *)swapchain, &desc))) return 0;
    return (desc.Flags & DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING) != 0;
#else
    (void)swapchain;
    return 0;
#endif
}

/* Opus multistream: float PCM, interleaved. */
void *nj_opus_open(int rate, int channels, int streams, int coupled, const unsigned char *mapping, int *error) {
    return opus_multistream_decoder_create(rate, channels, streams, coupled, mapping, error);
}

/* data NULL conceals a lost packet. Samples per channel, or a negative Opus error. */
int nj_opus_decode(void *decoder, const unsigned char *data, int length, float *pcm, int frame_size) {
    return opus_multistream_decode_float(decoder, data, data ? length : 0, pcm, frame_size, 0);
}

void nj_opus_close(void *decoder) {
    if (decoder) opus_multistream_decoder_destroy(decoder);
}
