#ifndef XCOC_RUST_H
#define XCOC_RUST_H
#include <stddef.h>
#include <stdint.h>
typedef struct { uint8_t *data; size_t length; } XcscFfiBytesV1;
typedef struct {
    uint32_t abi_revision;
    int32_t status;
    uint64_t value;
    XcscFfiBytesV1 bytes;
} XcscFfiResultV1;
// Initialized result must be released exactly once with xcsc_ffi_result_free_v1.
int32_t xcoc_call_v1(uint64_t handle, const uint8_t *input, size_t input_len, XcscFfiResultV1 *output);
int32_t xcoc_frame_v1(uint64_t handle, const uint8_t *frame, size_t frame_len, uint64_t timestamp_us, XcscFfiResultV1 *output);
int32_t xcsc_ffi_result_free_v1(XcscFfiResultV1 *output);
#endif
