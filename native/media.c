#include "media.h"

#include <errno.h>
#include <limits.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include <libavcodec/avcodec.h>
#include <libavcodec/bsf.h>
#include <libavformat/avformat.h>
#include <libavutil/avutil.h>
#include <libavutil/common.h>
#include <libavutil/dict.h>
#include <libavutil/log.h>
#include <libavutil/mem.h>
#include <libavutil/opt.h>
#include <libavutil/time.h>
#include <libavutil/version.h>

#define XCOC_OPEN_TIMEOUT_US INT64_C(12000000)
#define XCOC_IO_TIMEOUT_US INT64_C(5000000)
#define XCOC_MIN_AVFORMAT_VERSION AV_VERSION_INT(63, 1, 102)
#define XCOC_PENDING_PACKETS 1024U
#define XCOC_PENDING_BYTES (16U * 1024U * 1024U)

typedef struct MediaDeadline {
    int64_t until;
} MediaDeadline;

typedef struct JsonOutput {
    char *data;
    size_t capacity;
    size_t used;
    int failed;
} JsonOutput;

static void discard_log(void *context, int level, const char *format,
                        va_list arguments) {
    (void)context;
    (void)level;
    (void)format;
    (void)arguments;
}

static void silence_logs(void) {
    /* Even fatal libav messages can include URL userinfo or query tokens. */
    av_log_set_level(AV_LOG_QUIET);
    av_log_set_callback(discard_log);
}

static void deadline_after(MediaDeadline *deadline, int64_t duration) {
    deadline->until = av_gettime_relative() + duration;
}

static int deadline_expired(void *opaque) {
    const MediaDeadline *deadline = opaque;
    return av_gettime_relative() >= deadline->until;
}

static int has_protocol(const char *wanted, int output) {
    void *cursor = NULL;
    const char *name;
    while ((name = avio_enum_protocols(&cursor, output)) != NULL) {
        if (strcmp(name, wanted) == 0)
            return 1;
    }
    return 0;
}

static int has_option(const AVClass *class, const char *name) {
    return class != NULL &&
           av_opt_find(&class, name, NULL, 0, AV_OPT_SEARCH_FAKE_OBJ) != NULL;
}

int xcoc_media_check(void) {
    const AVInputFormat *input;
    const AVOutputFormat *output;
    silence_logs();
    /* Older RTSP implementations accepted neither tls_verify nor its safe
     * propagation to the internal TLS connection. Never downgrade to them. */
    if (avformat_version() < XCOC_MIN_AVFORMAT_VERSION)
        return 1;
    input = av_find_input_format("rtsp");
    output = av_guess_format("rtsp", NULL, NULL);
    if (input == NULL || output == NULL ||
        !has_option(input->priv_class, "tls_verify") ||
        !has_option(output->priv_class, "tls_verify") ||
        !has_option(input->priv_class, "rtsp_transport") ||
        !has_option(output->priv_class, "rtsp_transport") ||
        av_find_input_format("mpegts") == NULL ||
        av_guess_format("segment", NULL, NULL) == NULL ||
        av_guess_format("mp4", NULL, NULL) == NULL ||
        av_bsf_get_by_name("aac_adtstoasc") == NULL ||
        !has_protocol("tcp", 0) || !has_protocol("tcp", 1) ||
        !has_protocol("tls", 0) || !has_protocol("tls", 1) ||
        !has_protocol("http", 0) || !has_protocol("file", 1))
        return 1;
    return 0;
}

static int is_rtsp_url(const char *url) {
    return strncmp(url, "rtsp://", 7) == 0 ||
           strncmp(url, "rtsps://", 8) == 0;
}

static int open_input(AVFormatContext **context, const char *url, int rtsp,
                      MediaDeadline *deadline) {
    AVDictionary *options = NULL;
    const AVInputFormat *format = av_find_input_format(rtsp ? "rtsp" : "mpegts");
    int result = AVERROR(EINVAL);

    if (url == NULL || (rtsp != 0 && rtsp != 1) || format == NULL ||
        (rtsp && !is_rtsp_url(url)) ||
        (!rtsp && strncmp(url, "http://127.0.0.1:", 17) != 0))
        return result;
    *context = avformat_alloc_context();
    if (*context == NULL)
        return AVERROR(ENOMEM);
    (*context)->interrupt_callback.callback = deadline_expired;
    (*context)->interrupt_callback.opaque = deadline;

    if (av_dict_set(&options, "rw_timeout", "5000000", 0) < 0 ||
        av_dict_set(&options, "protocol_whitelist",
                    rtsp ? "tcp,tls" : "http,tcp", 0) < 0)
        goto done;
    if (rtsp) {
        if (av_dict_set(&options, "rtsp_transport", "tcp", 0) < 0 ||
            av_dict_set(&options, "timeout", "5000000", 0) < 0 ||
            av_dict_set(&options, "tls_verify", "1", 0) < 0)
            goto done;
    }

    result = avformat_open_input(context, url, format, &options);
    if (result < 0)
        goto done;
    /* Check critical options rather than silently ignoring a stripped or
     * incompatible build. Capability checks above happen before networking. */
    if (rtsp && (av_dict_get(options, "tls_verify", NULL, 0) != NULL ||
                 av_dict_get(options, "rtsp_transport", NULL, 0) != NULL)) {
        result = AVERROR(EINVAL);
        goto done;
    }
    result = avformat_find_stream_info(*context, NULL);
    if (result >= 0 &&
        ((*context)->nb_streams == 0 || deadline_expired(deadline)))
        result = AVERROR_INVALIDDATA;
done:
    av_dict_free(&options);
    return result;
}

static void json_append(JsonOutput *output, const char *format, ...) {
    va_list arguments;
    int count;
    if (output->failed)
        return;
    va_start(arguments, format);
    count = vsnprintf(output->data + output->used,
                      output->capacity - output->used, format, arguments);
    va_end(arguments);
    if (count < 0 || (size_t)count >= output->capacity - output->used) {
        output->failed = 1;
        return;
    }
    output->used += (size_t)count;
}

static void json_string(JsonOutput *output, const char *text) {
    const unsigned char *cursor = (const unsigned char *)text;
    json_append(output, "\"");
    while (*cursor != '\0' && !output->failed) {
        if (*cursor == '"' || *cursor == '\\')
            json_append(output, "\\%c", *cursor);
        else if (*cursor < 0x20)
            json_append(output, "\\u%04x", (unsigned int)*cursor);
        else
            json_append(output, "%c", *cursor);
        cursor++;
    }
    json_append(output, "\"");
}

int xcoc_media_probe(const char *input, int rtsp, char *json_output,
                     size_t capacity) {
    AVFormatContext *context = NULL;
    MediaDeadline deadline;
    JsonOutput output;
    int result = 1;

    if (json_output == NULL || capacity == 0)
        return 1;
    json_output[0] = '\0';
    if (xcoc_media_check() != 0 || avformat_network_init() < 0)
        return 1;
    deadline_after(&deadline, XCOC_OPEN_TIMEOUT_US);
    if (open_input(&context, input, rtsp, &deadline) < 0)
        goto done;
    output = (JsonOutput){json_output,
                          capacity < XCOC_MEDIA_PROBE_CAPACITY
                              ? capacity : XCOC_MEDIA_PROBE_CAPACITY,
                          0, 0};
    json_append(&output, "{\"streams\":[");
    for (unsigned int index = 0; index < context->nb_streams; index++) {
        const AVStream *stream = context->streams[index];
        const AVCodecParameters *codec = stream->codecpar;
        const char *type = av_get_media_type_string(codec->codec_type);
        json_append(&output, "%s{\"codec_type\":", index == 0 ? "" : ",");
        json_string(&output, type != NULL ? type : "unknown");
        json_append(&output, ",\"codec_name\":");
        json_string(&output, avcodec_get_name(codec->codec_id));
        if (codec->codec_type == AVMEDIA_TYPE_VIDEO)
            json_append(&output, ",\"width\":%d,\"height\":%d",
                        codec->width, codec->height);
        json_append(&output, ",\"r_frame_rate\":\"%d/%d\"}",
                    stream->r_frame_rate.num, stream->r_frame_rate.den);
        if (output.failed || deadline_expired(&deadline))
            goto done;
    }
    json_append(&output, "]}");
    if (!output.failed && !deadline_expired(&deadline))
        result = 0;
done:
    /* Keep the original deadline through RTSP teardown. The parent enforces
     * the independent hard timeout if an OS call cannot be interrupted. */
    avformat_close_input(&context);
    avformat_network_deinit();
    if (deadline_expired(&deadline))
        result = 1;
    if (result != 0)
        json_output[0] = '\0';
    return result;
}

static int copy_streams(AVFormatContext *output, const AVFormatContext *input) {
    for (unsigned int index = 0; index < input->nb_streams; index++) {
        const AVStream *source = input->streams[index];
        AVStream *target = avformat_new_stream(output, NULL);
        if (target == NULL)
            return AVERROR(ENOMEM);
        if (avcodec_parameters_copy(target->codecpar, source->codecpar) < 0 ||
            av_dict_copy(&target->metadata, source->metadata, 0) < 0)
            return AVERROR(ENOMEM);
        target->codecpar->codec_tag = 0;
        target->time_base = source->time_base;
        target->avg_frame_rate = source->avg_frame_rate;
        target->r_frame_rate = source->r_frame_rate;
        target->sample_aspect_ratio = source->sample_aspect_ratio;
        target->disposition = source->disposition;
        /* Keep the muxer's default stream ID. MPEG-TS PIDs are not portable
         * container IDs: RTSP treats IDs >=96 as explicit RTP payload types,
         * which would disagree with its generated SDP. Stream indexes, not
         * source container IDs, preserve the one-to-one track mapping. */
    }
    return 0;
}

static int prepare_aac_filters(AVBSFContext **filters, AVFormatContext *output,
                               const AVFormatContext *source) {
    const AVBitStreamFilter *filter = av_bsf_get_by_name("aac_adtstoasc");
    for (unsigned int index = 0; index < source->nb_streams; index++) {
        if (source->streams[index]->codecpar->codec_id != AV_CODEC_ID_AAC)
            continue;
        if (av_bsf_alloc(filter, &filters[index]) < 0 ||
            avcodec_parameters_copy(filters[index]->par_in,
                                    source->streams[index]->codecpar) < 0)
            return AVERROR(ENOMEM);
        filters[index]->time_base_in = source->streams[index]->time_base;
        if (av_bsf_init(filters[index]) < 0 ||
            avcodec_parameters_copy(output->streams[index]->codecpar,
                                    filters[index]->par_out) < 0)
            return AVERROR_INVALIDDATA;
        output->streams[index]->codecpar->codec_tag = 0;
    }
    return 0;
}

static int missing_aac_configuration(const AVFormatContext *output) {
    for (unsigned int index = 0; index < output->nb_streams; index++) {
        const AVCodecParameters *codec = output->streams[index]->codecpar;
        if (codec->codec_id == AV_CODEC_ID_AAC && codec->extradata_size == 0)
            return 1;
    }
    return 0;
}

static int filter_aac_packet(AVBSFContext **filters, AVFormatContext *output,
                             AVPacket *packet) {
    AVBSFContext *filter = filters[packet->stream_index];
    AVCodecParameters *codec = output->streams[packet->stream_index]->codecpar;
    const uint8_t *configuration;
    size_t configuration_size = 0;
    uint8_t *copy;
    if (filter == NULL)
        return 0;
    /* aac_adtstoasc is a one-packet-in, one-packet-out filter with no delay.
     * Keeping it active also strips ADTS after SDP advertises the ASC. */
    if (av_bsf_send_packet(filter, packet) < 0 ||
        av_bsf_receive_packet(filter, packet) < 0)
        return AVERROR_INVALIDDATA;
    configuration = av_packet_get_side_data(packet, AV_PKT_DATA_NEW_EXTRADATA,
                                             &configuration_size);
    if (configuration == NULL)
        return 0;
    if (configuration_size > INT_MAX - AV_INPUT_BUFFER_PADDING_SIZE)
        return AVERROR_INVALIDDATA;
    copy = av_mallocz(configuration_size + AV_INPUT_BUFFER_PADDING_SIZE);
    if (copy == NULL)
        return AVERROR(ENOMEM);
    memcpy(copy, configuration, configuration_size);
    av_freep(&codec->extradata);
    codec->extradata = copy;
    codec->extradata_size = (int)configuration_size;
    return 0;
}

static int read_packet(AVFormatContext *source, AVPacket *packet,
                       MediaDeadline *deadline) {
    int status;
    do {
        status = av_read_frame(source, packet);
        if (status == AVERROR(EAGAIN))
            av_usleep(1000);
    } while (status == AVERROR(EAGAIN) && !deadline_expired(deadline));
    return deadline_expired(deadline) ? AVERROR(ETIMEDOUT) : status;
}

static int valid_packet_stream(const AVPacket *packet,
                                const AVFormatContext *source,
                                const AVFormatContext *output) {
    return packet->stream_index >= 0 &&
           (unsigned int)packet->stream_index < output->nb_streams &&
           source->nb_streams == output->nb_streams;
}

int xcoc_media_run(const char *input, int rtsp, const char *destination,
                   int record) {
    AVFormatContext *source = NULL;
    AVFormatContext *output = NULL;
    AVPacket *packet = NULL;
    AVBSFContext **filters = NULL;
    AVPacket *pending[XCOC_PENDING_PACKETS] = {0};
    unsigned int pending_count = 0;
    unsigned int pending_index = 0;
    size_t pending_bytes = 0;
    int64_t input_epoch = 0;
    AVDictionary *options = NULL;
    MediaDeadline deadline;
    int header_written = 0;
    int result = 1;
    int status;

    if (destination == NULL || destination[0] == '\0' ||
        (record != 0 && record != 1) ||
        (!record && !is_rtsp_url(destination)) || xcoc_media_check() != 0 ||
        avformat_network_init() < 0)
        return 1;
    deadline_after(&deadline, XCOC_OPEN_TIMEOUT_US);
    if (open_input(&source, input, rtsp, &deadline) < 0)
        goto done;
    if (source->start_time != AV_NOPTS_VALUE)
        input_epoch = source->start_time;
    if (avformat_alloc_output_context2(&output, NULL,
                                      record ? "segment" : "rtsp",
                                      destination) < 0 || output == NULL)
        goto done;
    output->interrupt_callback.callback = deadline_expired;
    output->interrupt_callback.opaque = &deadline;
    /* Segment forwards AUTO_BSF to its underlying muxer and moves necessary
     * filters to the outer streams. This handles AAC ADTS-to-ASC and the
     * muxer's H.264/HEVC representations without losing audio or other tracks. */
    output->flags |= AVFMT_FLAG_AUTO_BSF;
    if (copy_streams(output, source) < 0)
        goto done;
    packet = av_packet_alloc();
    if (packet == NULL)
        goto done;
    if (!record) {
        filters = av_calloc(output->nb_streams, sizeof(*filters));
        if (filters == NULL || prepare_aac_filters(filters, output, source) < 0)
            goto done;
        /* RTP's SDP needs AAC AudioSpecificConfig before write_header. MPEG-TS
         * ADTS often has none even after find_stream_info. Extract it using the
         * public bitstream-filter API; retain every packet read along the way.
         * Missing/malicious tracks cannot cause unbounded pre-header buffering. */
        deadline_after(&deadline, XCOC_OPEN_TIMEOUT_US);
        while (missing_aac_configuration(output)) {
            if (pending_count == XCOC_PENDING_PACKETS ||
                read_packet(source, packet, &deadline) < 0 ||
                !valid_packet_stream(packet, source, output) ||
                filter_aac_packet(filters, output, packet) < 0 ||
                packet->size < 0 ||
                (size_t)packet->size > XCOC_PENDING_BYTES - pending_bytes)
                goto done;
            pending[pending_count] = av_packet_alloc();
            if (pending[pending_count] == NULL)
                goto done;
            pending_bytes += (size_t)packet->size;
            av_packet_move_ref(pending[pending_count++], packet);
        }
    }
    if (record) {
        if (av_dict_set(&options, "segment_format", "mp4", 0) < 0 ||
            av_dict_set(&options, "segment_time", "900", 0) < 0 ||
            av_dict_set(&options, "reset_timestamps", "1", 0) < 0 ||
            av_dict_set(&options, "strftime", "1", 0) < 0)
            goto done;
    } else {
        if (av_dict_set(&options, "rtsp_transport", "tcp", 0) < 0 ||
            av_dict_set(&options, "tls_verify", "1", 0) < 0)
            goto done;
    }
    /* Both selected muxers own their I/O (AVFMT_NOFILE). In particular,
     * pre-opening the segment filename would write to an unexpanded pattern. */
    if (!(output->oformat->flags & AVFMT_NOFILE))
        goto done;
    deadline_after(&deadline, XCOC_OPEN_TIMEOUT_US);
    if (avformat_write_header(output, &options) < 0)
        goto done;
    header_written = 1;
    if (av_dict_count(options) != 0 || deadline_expired(&deadline))
        goto done;
    av_dict_free(&options);
    for (;;) {
        if (pending_index < pending_count) {
            av_packet_move_ref(packet, pending[pending_index++]);
        } else {
            deadline_after(&deadline, XCOC_IO_TIMEOUT_US);
            status = read_packet(source, packet, &deadline);
            if (status == AVERROR_EOF) {
                result = 0;
                break;
            }
            if (status < 0 || !valid_packet_stream(packet, source, output) ||
                (filters != NULL && filter_aac_packet(filters, output, packet) < 0))
                goto done;
        }
        /* A newly discovered stream cannot be added after the output header.
         * Fail visibly instead of silently omitting it like many examples do. */
        if (!valid_packet_stream(packet, source, output))
            goto done;
        const AVRational packet_time_base =
            filters != NULL && filters[packet->stream_index] != NULL
                ? filters[packet->stream_index]->time_base_out
                : source->streams[packet->stream_index]->time_base;
        const int64_t offset = av_rescale_q(input_epoch, AV_TIME_BASE_Q,
                                           packet_time_base);
        /* Match ffmpeg's default (without -copyts): one input-wide epoch keeps
         * A/V offsets intact. Segment reset_timestamps resets later segments,
         * but does not by itself normalize the first segment's source epoch. */
        if (packet->pts != AV_NOPTS_VALUE)
            packet->pts = av_sat_sub64(packet->pts, offset);
        if (packet->dts != AV_NOPTS_VALUE)
            packet->dts = av_sat_sub64(packet->dts, offset);
        av_packet_rescale_ts(packet, packet_time_base,
                             output->streams[packet->stream_index]->time_base);
        packet->pos = -1;
        deadline_after(&deadline, XCOC_IO_TIMEOUT_US);
        status = av_interleaved_write_frame(output, packet);
        /* This API takes packet ownership, including on error. */
        if (status < 0 || deadline_expired(&deadline))
            goto done;
    }
done:
    av_packet_free(&packet);
    for (unsigned int index = 0; index < pending_count; index++)
        av_packet_free(&pending[index]);
    if (filters != NULL) {
        for (unsigned int index = 0; index < output->nb_streams; index++)
            av_bsf_free(&filters[index]);
        av_freep(&filters);
    }
    av_dict_free(&options);
    if (header_written) {
        deadline_after(&deadline, XCOC_IO_TIMEOUT_US);
        if (av_write_trailer(output) < 0 || deadline_expired(&deadline))
            result = 1;
    }
    /* Free muxer-owned state after trailer/failed-header cleanup. Segment also
     * supplies a deinitializer for partially opened nested output contexts. */
    deadline_after(&deadline, XCOC_IO_TIMEOUT_US);
    avformat_free_context(output);
    deadline_after(&deadline, XCOC_IO_TIMEOUT_US);
    avformat_close_input(&source);
    avformat_network_deinit();
    return result;
}
