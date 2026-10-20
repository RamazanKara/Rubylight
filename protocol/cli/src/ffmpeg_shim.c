/* A narrow libavcodec wrapper for nightjar-cli, so Rust never touches FFmpeg structs.
 * The decoder setup follows Rubylight's rust/tests/moonlight_client.c: slice threads, low
 * delay, libdav1d with one frame of delay, optional hardware decoding with readback. */
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <libavcodec/avcodec.h>
#include <libavutil/hwcontext.h>
#include <libavutil/pixdesc.h>

typedef struct nj_decoder {
    AVCodecContext *context;
    AVPacket *packet;
    AVFrame *decoded;
    AVFrame *frame;
    enum AVPixelFormat hardware_format;
} nj_decoder;

static enum AVPixelFormat select_hardware_format(AVCodecContext *context, const enum AVPixelFormat *formats) {
    nj_decoder *decoder = context->opaque;
    for (const enum AVPixelFormat *format = formats; *format != AV_PIX_FMT_NONE; format++)
        if (*format == decoder->hardware_format) return *format;
    return AV_PIX_FMT_NONE;
}

void nj_decoder_close(nj_decoder *decoder) {
    if (!decoder) return;
    avcodec_free_context(&decoder->context);
    av_packet_free(&decoder->packet);
    av_frame_free(&decoder->decoded);
    av_frame_free(&decoder->frame);
    av_free(decoder);
}

/* codec: 0 H.264, 1 HEVC, 2 AV1. hardware may be NULL. Writes the codec name and the
 * hardware pixel format name (empty without hardware). Returns NULL with *error set. */
nj_decoder *nj_decoder_open(int codec, int threads, const char *hardware, const char *device,
                            char *name, int name_length, char *hardware_name, int hardware_length,
                            int *active_thread_type, int *delay, int *error) {
    enum AVCodecID id = codec == 0 ? AV_CODEC_ID_H264 : codec == 1 ? AV_CODEC_ID_HEVC : AV_CODEC_ID_AV1;
    const AVCodec *found = (hardware && id == AV_CODEC_ID_AV1) ? avcodec_find_decoder_by_name("av1") : NULL;
    if (!found) found = avcodec_find_decoder(id);
    *error = -1;
    if (!found) return NULL;
    nj_decoder *decoder = av_mallocz(sizeof(*decoder));
    if (!decoder) return NULL;
    decoder->hardware_format = AV_PIX_FMT_NONE;
    decoder->context = avcodec_alloc_context3(found);
    decoder->packet = av_packet_alloc();
    decoder->decoded = av_frame_alloc();
    decoder->frame = av_frame_alloc();
    if (!decoder->context || !decoder->packet || !decoder->decoded || !decoder->frame) goto fail;
    decoder->context->opaque = decoder;
    hardware_name[0] = 0;
    if (hardware) {
        enum AVHWDeviceType type = av_hwdevice_find_type_by_name(hardware);
        if (type == AV_HWDEVICE_TYPE_NONE) goto fail;
        for (int i = 0;; i++) {
            const AVCodecHWConfig *config = avcodec_get_hw_config(found, i);
            if (!config) goto fail;
            if (config->device_type == type && (config->methods & AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX)) {
                decoder->hardware_format = config->pix_fmt;
                break;
            }
        }
        if (av_hwdevice_ctx_create(&decoder->context->hw_device_ctx, type, device, NULL, 0) < 0) goto fail;
        decoder->context->get_format = select_hardware_format;
        decoder->context->extra_hw_frames = 8;
        snprintf(hardware_name, hardware_length, "%s", av_get_pix_fmt_name(decoder->hardware_format));
    }
    decoder->context->thread_count = threads;
    decoder->context->thread_type = FF_THREAD_SLICE;
    decoder->context->flags |= AV_CODEC_FLAG_LOW_DELAY;
    AVDictionary *options = NULL;
    if (strcmp(found->name, "libdav1d") == 0) av_dict_set(&options, "max_frame_delay", "1", 0);
    *error = avcodec_open2(decoder->context, found, &options);
    av_dict_free(&options);
    snprintf(name, name_length, "%s", found->name);
    *active_thread_type = decoder->context->active_thread_type;
    *delay = decoder->context->delay;
    if (*error < 0) goto fail;
    return decoder;
fail:
    nj_decoder_close(decoder);
    return NULL;
}

/* Returns avcodec_send_packet's result. */
int nj_decoder_send(nj_decoder *decoder, const uint8_t *data, int length, int64_t pts) {
    av_packet_unref(decoder->packet);
    if (av_new_packet(decoder->packet, length) < 0) return -1;
    memcpy(decoder->packet->data, data, length);
    decoder->packet->pts = pts;
    return avcodec_send_packet(decoder->context, decoder->packet);
}

/* 0: a frame is ready (read back to memory for hardware decoding); 1: none (EAGAIN or EOF);
 * 2: a hardware frame could not be read back; negative: a decode error. */
int nj_decoder_receive(nj_decoder *decoder) {
    av_frame_unref(decoder->frame);
    av_frame_unref(decoder->decoded);
    int result = avcodec_receive_frame(decoder->context, decoder->decoded);
    if (result == AVERROR(EAGAIN) || result == AVERROR_EOF) return 1;
    if (result < 0) return result;
    if (decoder->hardware_format != AV_PIX_FMT_NONE) {
        if (decoder->decoded->format != decoder->hardware_format ||
            av_hwframe_transfer_data(decoder->frame, decoder->decoded, 0) < 0 ||
            av_frame_copy_props(decoder->frame, decoder->decoded) < 0)
            return 2;
    } else {
        av_frame_move_ref(decoder->frame, decoder->decoded);
    }
    return 0;
}

int nj_frame_width(const nj_decoder *d) { return d->frame->width; }
int nj_frame_height(const nj_decoder *d) { return d->frame->height; }
int nj_frame_format(const nj_decoder *d) { return d->frame->format; }
int nj_frame_primaries(const nj_decoder *d) { return d->frame->color_primaries; }
int nj_frame_transfer(const nj_decoder *d) { return d->frame->color_trc; }
int64_t nj_frame_pts(const nj_decoder *d) { return d->frame->pts; }

/* Crops the visible size (AMD AV1 surfaces carry alignment padding). */
void nj_frame_crop(nj_decoder *d, int width, int height) {
    d->frame->width = width;
    d->frame->height = height;
}

int nj_frame_depth(const nj_decoder *d) {
    const AVPixFmtDescriptor *desc = av_pix_fmt_desc_get(d->frame->format);
    return desc ? desc->comp[0].depth : 0;
}

const char *nj_frame_format_name(const nj_decoder *d) {
    const char *name = av_get_pix_fmt_name(d->frame->format);
    return name ? name : "unknown";
}

unsigned nj_frame_luma(const nj_decoder *d, int x, int y) {
    const AVFrame *frame = d->frame;
    const AVPixFmtDescriptor *desc = av_pix_fmt_desc_get(frame->format);
    if (!desc || desc->comp[0].plane != 0 || x < 0 || y < 0 || x >= frame->width || y >= frame->height) return 0;
    const AVComponentDescriptor *component = &desc->comp[0];
    const unsigned char *pixel = frame->data[0] + y * frame->linesize[0] + x * component->step + component->offset;
    unsigned value = pixel[0];
    if (component->depth > 8) value |= (unsigned)pixel[1] << 8;
    return (value >> component->shift) & ((1u << component->depth) - 1u);
}

/* The decoded picture for colour comparisons: a text header, then each plane's rows. */
int nj_frame_dump(const nj_decoder *d, const char *path) {
    const AVFrame *frame = d->frame;
    const AVPixFmtDescriptor *desc = av_pix_fmt_desc_get(frame->format);
    FILE *file = fopen(path, "wb");
    if (!file || !desc) {
        if (file) fclose(file);
        return -1;
    }
    int bytes = (desc->comp[0].depth + 7) / 8;
    fprintf(file, "BPFRAME %d %d %s %d %d %d %d\n", frame->width, frame->height, desc->name, desc->comp[0].depth,
            frame->color_range, frame->colorspace, desc->nb_components);
    for (int plane = 0; plane < 3 && frame->data[plane]; plane++) {
        int w = plane ? AV_CEIL_RSHIFT(frame->width, desc->log2_chroma_w) : frame->width;
        int h = plane ? AV_CEIL_RSHIFT(frame->height, desc->log2_chroma_h) : frame->height;
        int step = (desc->flags & AV_PIX_FMT_FLAG_PLANAR) || plane == 0 ? 1 : 2;
        for (int y = 0; y < h; y++) fwrite(frame->data[plane] + (size_t)y * frame->linesize[plane], 1, (size_t)w * bytes * step, file);
    }
    fclose(file);
    return 0;
}
