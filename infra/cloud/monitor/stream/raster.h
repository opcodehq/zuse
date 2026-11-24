#pragma once

#include <X11/Xlib.h>
#include <stdbool.h>
#include <stdint.h>

enum { PATCH_LIMIT = 1024 * 1024 };
struct raster_encoder;
// Buffers live for the life of the per-display capture process.
struct raster_encoder *raster_create(int width, int height, bool extended);
size_t raster_recovery(struct raster_encoder *, const XImage *,
                       const uint8_t **);
size_t raster_compress(struct raster_encoder *, const uint8_t *, size_t,
                       const uint8_t **);
