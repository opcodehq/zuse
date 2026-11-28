// Encode exact desktop patches and indexed recovery images.
#include "raster.h"
#include <endian.h>
#include <libdeflate.h>
#include <lz4.h>
#include <stdlib.h>
#include <string.h>

struct raster_encoder {
  uint8_t *primary, *secondary, *indices;
  struct libdeflate_compressor *fast, *recovery;
};

struct raster_encoder *raster_create(int width, int height, bool extended) {
  struct raster_encoder *e = calloc(1, sizeof(*e));
  if (!e)
    return NULL;
  e->primary = malloc(4 + LZ4_compressBound(PATCH_LIMIT));
  if (!e->primary)
    return NULL;
  if (extended) {
    e->secondary = malloc(PATCH_LIMIT + 4);
    e->indices = malloc((size_t)width * height);
    e->fast = libdeflate_alloc_compressor(1);
    e->recovery = libdeflate_alloc_compressor(6);
    if (!e->secondary || !e->indices || !e->fast || !e->recovery)
      return NULL;
  }
  return e;
}

extern size_t poof_palette(const uint32_t *, size_t, size_t, size_t, uint32_t *,
                           uint8_t *);

extern size_t poof_fold_rows(const uint8_t *, size_t, size_t, uint8_t *,
                             size_t);

// Packet header: width, height and palette length (little-endian u16),
// followed by RGBA entries and a zlib-compressed plane of palette indices.
size_t raster_recovery(struct raster_encoder *e, const XImage *image,
                       const uint8_t **output) {
  uint8_t *indices = e->indices, *folded = e->secondary, *packet = e->primary;
  *output = packet;
  uint32_t palette[256];
  size_t n = poof_palette((const uint32_t *)image->data, image->bytes_per_line,
                          image->width, image->height, palette, indices);
  if (!n)
    return 0;
  size_t cells = (size_t)image->width * image->height;
  size_t dictionary =
      poof_fold_rows(indices, image->width, image->height, folded, PATCH_LIMIT);
  uint16_t format = (uint16_t)n | (dictionary ? UINT16_C(0x8000) : 0);
  uint16_t header[] = {htole16(image->width), htole16(image->height),
                       htole16(format)};
  size_t prefix = sizeof(header) + n * 4;
  size_t bytes = libdeflate_zlib_compress(
      e->recovery, dictionary ? folded : indices,
      dictionary ? dictionary : cells, packet + prefix, 96 * 1024 - prefix);
  if (!bytes)
    return 0;
  memcpy(packet, header, sizeof(header));
  memcpy(packet + sizeof(header), palette, n * 4);
  return prefix + bytes;
}

size_t raster_compress(struct raster_encoder *e, const uint8_t *patch,
                       size_t patch_size, const uint8_t **output) {
  uint8_t *compressed = e->primary, *deflated = e->secondary;
  uint32_t raw_length = htole32((uint32_t)patch_size);
  memcpy(compressed, &raw_length, 4);
  int packed = LZ4_compress_default((const char *)patch, (char *)compressed + 4,
                                    patch_size, LZ4_compressBound(patch_size));
  if (!packed)
    return 0;
  // Spend extra compression time only on updates large enough to
  // queue on a constrained link. The decoded span format is identical.
  uint8_t *payload = compressed;
  if (e->fast && packed > 1500) {
    size_t limit = (size_t)packed * 7 / 8;
    if (limit > PATCH_LIMIT)
      limit = PATCH_LIMIT;
    size_t smaller = libdeflate_zlib_compress(e->fast, patch, patch_size,
                                              deflated + 4, limit);
    if (smaller) {
      raw_length = htole32((uint32_t)patch_size | UINT32_C(0x80000000));
      memcpy(deflated, &raw_length, 4);
      payload = deflated;
      packed = (int)smaller;
    }
  }
  *output = payload;
  return 4 + (size_t)packed;
}
