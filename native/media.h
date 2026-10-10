#ifndef XCOC_MEDIA_H
#define XCOC_MEDIA_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

#define XCOC_MEDIA_PROBE_CAPACITY (256U * 1024U)

/* These functions run only in an isolated, disposable media worker process.
 * They silence libav's process-global logger. Callers retain ownership of all
 * arguments and must supply valid NUL-terminated strings. Normal cancellation
 * must request finalization before a bounded kill/reap fallback. URLs are never
 * passed to another process or logged.
 * All functions return 0 on success and 1 on any failure, without error text.
 */
int xcoc_media_check(void);

/* Write a NUL-terminated ffprobe-compatible {"streams":[...]} document.
 * Capacity includes the terminator and is capped at XCOC_MEDIA_PROBE_CAPACITY.
 * On failure, an available output buffer is reset to an empty string.
 * Input opening and discovery share a 12-second interrupt deadline; the parent
 * must also enforce a hard process timeout, including uninterruptible OS work.
 */
int xcoc_media_probe(const char *input, int rtsp, char *json_output,
                     size_t capacity);

/* Copy every input stream without transcoding. rtsp selects an RTSP(S) source;
 * otherwise input is the protected loopback HTTP MPEG-TS capture relay.
 * record=0 publishes over RTSP(S)/TCP with TLS verification enabled.
 * record=1 writes 900-second MP4 segments, resetting timestamps and expanding
 * the caller's strftime destination pattern. No stream is silently discarded.
 * Opening/discovery has a 12-second deadline; streaming operations have fresh
 * 5-second deadlines, so a healthy continuous stream has no total time limit.
 * cancelled is a thread-safe, nonblocking callback, or NULL. Cancellation
 * interrupts input/output I/O, then bounded trailer cleanup runs without the
 * cancellation callback so an ordinary stop produces a playable final segment.
 */
int xcoc_media_run(const char *input, int rtsp, const char *destination,
                   int record, int (*cancelled)(void));

#ifdef __cplusplus
}
#endif

#endif
