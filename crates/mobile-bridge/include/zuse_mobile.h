#ifndef ZUSE_MOBILE_H
#define ZUSE_MOBILE_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
/* Input remains caller-owned. Free the returned UTF-8 JSON string exactly once. */
char *zuse_mobile_request(const uint8_t *input, size_t len);
void zuse_mobile_free(char *value);
#ifdef __cplusplus
}
#endif
#endif
