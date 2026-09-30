#ifndef REMOVENT_MOBILE_H
#define REMOVENT_MOBILE_H
#include <stdint.h>
#include <stddef.h>

// Serialized calls (Swift MainActor). No callbacks or borrowed output buffers.
// Free every returned string/frame/audio object with its matching function.
typedef struct Engine RMEngine;
typedef struct {
    uint8_t *data;
    size_t len;
    uint32_t width;
    uint32_t height;
    uint64_t generation;
} RMFrame;
typedef struct {
    int16_t *data;
    size_t len;
    uint32_t sample_rate;
    uint8_t channels;
    uint64_t generation;
} RMAudio;
RMEngine *rm_create(const char *path, const char *name, char **error);
char *rm_call(RMEngine *, const char *command);
// Handle-independent, serialized by the Rust storage lock.
char *rm_sync(const char *path, const char *command);
char *rm_poll_event(RMEngine *);
RMFrame *rm_take_frame(RMEngine *);
RMAudio *rm_take_audio(RMEngine *);
void rm_string_free(char *);
void rm_frame_free(RMFrame *);
void rm_audio_free(RMAudio *);
void rm_destroy(RMEngine *);
#endif
