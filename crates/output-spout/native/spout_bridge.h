#ifndef LATENTDECK_SPOUT_BRIDGE_H
#define LATENTDECK_SPOUT_BRIDGE_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#define LATENTDECK_SPOUT_NOEXCEPT noexcept
#else
#define LATENTDECK_SPOUT_NOEXCEPT
#endif

typedef struct latentdeck_spout_sender latentdeck_spout_sender;

enum latentdeck_spout_result {
    LATENTDECK_SPOUT_OK = 0,
    LATENTDECK_SPOUT_INVALID_ARGUMENT = 1,
    LATENTDECK_SPOUT_OPEN_FAILED = 2,
    LATENTDECK_SPOUT_NAME_FAILED = 3,
    LATENTDECK_SPOUT_RESOURCE_MISMATCH = 4,
    LATENTDECK_SPOUT_WRAP_FAILED = 5,
    LATENTDECK_SPOUT_SEND_FAILED = 6,
    LATENTDECK_SPOUT_RESOURCE_LIMIT = 7,
    LATENTDECK_SPOUT_DEVICE_MISMATCH = 8,
    LATENTDECK_SPOUT_INTERNAL_ERROR = 9
};

typedef struct latentdeck_spout_status {
    uint32_t schema;
    uint32_t published;
    uint32_t width;
    uint32_t height;
    uint32_t format;
    int64_t spout_frame;
    char active_name[256];
} latentdeck_spout_status;

int32_t latentdeck_spout_sender_open(
    void* d3d12_device,
    void* direct_command_queue,
    const char* sender_name,
    uint32_t width,
    uint32_t height,
    uint32_t dxgi_format,
    latentdeck_spout_sender** out_sender,
    latentdeck_spout_status* out_status) LATENTDECK_SPOUT_NOEXCEPT;

int32_t latentdeck_spout_sender_set_name(
    latentdeck_spout_sender* sender,
    const char* sender_name,
    latentdeck_spout_status* out_status) LATENTDECK_SPOUT_NOEXCEPT;

int32_t latentdeck_spout_sender_release(
    latentdeck_spout_sender* sender,
    latentdeck_spout_status* out_status) LATENTDECK_SPOUT_NOEXCEPT;

int32_t latentdeck_spout_sender_send_resource(
    latentdeck_spout_sender* sender,
    void* d3d12_resource,
    uint32_t initial_resource_state,
    latentdeck_spout_status* out_status) LATENTDECK_SPOUT_NOEXCEPT;

void latentdeck_spout_sender_destroy(latentdeck_spout_sender* sender)
    LATENTDECK_SPOUT_NOEXCEPT;

#ifdef __cplusplus
}
#endif

#undef LATENTDECK_SPOUT_NOEXCEPT

#endif
