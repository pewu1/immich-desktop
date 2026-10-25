#pragma once
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
typedef struct {
    void *(*retain)(void *);
    void (*release)(void *);
    int (*copy_file)(void *, const char *);
    int (*read_file)(void *, uint64_t, unsigned char *, size_t, size_t *);
    int (*file_size)(void *, uint64_t *);
    void (*ended)(void *, int);
    int (*file_path)(void *, char *, size_t);
    void (*failed)(void *, const char *);
    int (*ready)(void *);
} ImmichCallbacks;
// Takes ownership of one context reference, including on failure. Call on UI thread.
int immich_start_drag(void *window, const char *name, const unsigned char *png, size_t png_len,
    const unsigned char *rgba, uint32_t width, uint32_t height, void *context, ImmichCallbacks callbacks);
int immich_mouse_down(void);
#ifdef __cplusplus
}
#endif
