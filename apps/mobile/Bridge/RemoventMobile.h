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
    // Borrowed CVPixelBuffer until rm_frame_free. Retain for asynchronous GPU use.
    void *surface;
    int64_t pts_us;
} RMFrame;
// Renderer is retained independently of RMEngine and exclusively used by the
// audio callback. Free only after releasing its AVAudioSourceNode callback.
typedef struct AudioRenderer RMAudioRenderer;
RMAudioRenderer *rm_audio_renderer_create(RMEngine *, uint64_t generation);
void rm_audio_render(RMAudioRenderer *, float *left, float *right, size_t frames, uint32_t rate);
void rm_audio_renderer_free(RMAudioRenderer *);
RMEngine *rm_create(const char *path, const char *name, char **error);
char *rm_call(RMEngine *, const char *command);
// Handle-independent, serialized by the Rust storage lock.
char *rm_sync(const char *path, const char *command);
// Handle-independent Keychain access; run off MainActor.
char *rm_native_credentials(const char *path, const char *command);
// Numeric Bonjour hosts only; shared local-interface filter. Null means hidden.
char *rm_discovery_host(const char *addresses);
char *rm_poll_event(RMEngine *);
RMFrame *rm_take_frame(RMEngine *);
void rm_string_free(char *);
void rm_frame_free(RMFrame *);
void rm_destroy(RMEngine *);
#endif
