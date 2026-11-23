// Capture the 24-bit Xvfb root into a private shared-memory image. Convert
// straight into x264's input planes and emit each complete access unit as it
// becomes ready.
#include <X11/Xlib.h>
#include <X11/Xutil.h>
#include <X11/extensions/XShm.h>
#include <X11/extensions/Xdamage.h>
#include <endian.h>
#include <errno.h>
#include <libyuv/convert.h>
#include <lz4.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/shm.h>
#include <time.h>
#include <x264.h>

enum { PATCH_LIMIT = 1024 * 1024 };
extern size_t zuse_patch(const uint32_t *, const uint32_t *, size_t, int16_t *,
                         uint8_t *, size_t);

static uint64_t monotonic_ns(void) {
  struct timespec t;
  clock_gettime(CLOCK_MONOTONIC, &t);
  return (uint64_t)t.tv_sec * 1000000000 + t.tv_nsec;
}

static XImage *capture_buffer(Display *display, int screen, int width,
                              int height, XShmSegmentInfo *segment) {

  XImage *image = XShmCreateImage(display, DefaultVisual(display, screen),
                                  DefaultDepth(display, screen), ZPixmap, NULL,
                                  segment, width, height);
  if (!image || image->bits_per_pixel != 32 || image->byte_order != LSBFirst ||
      image->red_mask != 0xff0000 || image->green_mask != 0xff00 ||
      image->blue_mask != 0xff)
    return NULL;
  segment->shmid = shmget(IPC_PRIVATE, (size_t)image->bytes_per_line * height,
                          IPC_CREAT | 0600);
  if (segment->shmid == -1)
    return NULL;
  image->data = segment->shmaddr = shmat(segment->shmid, NULL, 0);
  // Linux allows Xvfb to attach by id while this process holds the segment.
  shmctl(segment->shmid, IPC_RMID, NULL);
  if (image->data == (void *)-1 || !XShmAttach(display, segment))
    return NULL;
  XSync(display, False);
  return image;
}

int main(void) {
  Display *display = XOpenDisplay(NULL);
  if (!display)
    return 1;
  int screen = DefaultScreen(display);
  Window root = RootWindow(display, screen);
  int width = DisplayWidth(display, screen),
      height = DisplayHeight(display, screen);
  if (width != 1920 || height != 1080 || !XShmQueryExtension(display))
    return 1;
  XShmSegmentInfo segments[2] = {0};
  XImage *buffers[2];
  for (int i = 0; i < 2; i++) {
    buffers[i] = capture_buffer(display, screen, width, height, &segments[i]);
    if (!buffers[i])
      return 1;
  }
  int index = 0;

  x264_param_t config;
  if (x264_param_default_preset(&config, "ultrafast", "zerolatency") < 0)
    return 1;
  config.i_width = width;
  config.i_height = height;
  config.i_csp = X264_CSP_I420;
  config.i_fps_num = 60;
  config.i_fps_den = 1;
  config.i_threads = 2;
  config.i_keyint_max = 30;
  config.b_repeat_headers = 1;
  config.b_annexb = 1;
  config.i_log_level = X264_LOG_ERROR;
  config.rc.i_rc_method = X264_RC_ABR;
  config.rc.i_bitrate = 4000;
  if (x264_param_apply_profile(&config, "baseline") < 0)
    return 1;
  x264_t *encoder = x264_encoder_open(&config);
  x264_picture_t input, output;
  if (!encoder || x264_picture_alloc(&input, X264_CSP_I420, width, height) < 0)
    return 1;

  uint8_t *patch = malloc(PATCH_LIMIT);
  uint8_t *compressed = malloc(4 + LZ4_compressBound(PATCH_LIMIT));
  if (!patch || !compressed)
    return 1;

  int16_t last_shift = 0;
  int sent_patch = 0;
  int damage_event, damage_error;
  if (!XDamageQueryExtension(display, &damage_event, &damage_error))
    return 1;
  Damage damage = XDamageCreate(display, root, XDamageReportNonEmpty);
  XSync(display, False);
  struct pollfd connection = {ConnectionNumber(display), POLLIN, 0};
  uint64_t start = monotonic_ns(), last_key = 0, last_capture = 0;
  int last_x = -1, last_y = -1;
  for (;;) {
    // Wake on a painted frame; the short timeout also tracks cursor-only
    // motion.
    if (!XPending(display) && last_capture) {
      if (poll(&connection, 1, 16) < 0 && errno != EINTR)
        return 1;
    }
    int dirty = 0;
    while (XPending(display)) {
      XEvent event;
      XNextEvent(display, &event);
      if (event.type == damage_event + XDamageNotify)
        dirty = 1;
    }
    Window child, returned_root;
    int x = -1, y = -1, wx, wy;
    unsigned int mask;
    int pointer = XQueryPointer(display, root, &returned_root, &child, &x, &y,
                                &wx, &wy, &mask);
    uint64_t now = monotonic_ns();
    int key_due = !last_key || now - last_key >= 1000000000;
    if (!key_due && !dirty && x == last_x && y == last_y)
      continue;
    // Coalesce very rapid updates without repeatedly encoding a partial
    // repaint.
    uint64_t deadline = last_capture + 1000000000 / 120;
    if (now < deadline) {
      struct timespec t = {deadline / 1000000000, deadline % 1000000000};
      clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, &t, NULL);
    }
    if (dirty)
      XDamageSubtract(display, damage, None, None);
    last_capture = monotonic_ns();
    last_x = x;
    last_y = y;
    XImage *image = buffers[index];
    const uint8_t *previous = (const uint8_t *)buffers[index ^ 1]->data;
    if (!XShmGetImage(display, root, image, 0, 0, AllPlanes))
      return 1;
    if (pointer) {
      for (int dy = 0; dy < 16 && y + dy < height && y + dy >= 0; dy++) {
        for (int dx = 0; dx <= dy / 2 && x + dx < width && x + dx >= 0; dx++) {
          uint32_t *row =
              (uint32_t *)(image->data + (y + dy) * image->bytes_per_line);
          row[x + dx] =
              dx == 0 || dx == dy / 2 || dy == 15 ? 0x101010 : 0xffffff;
        }
      }
    }
    uint64_t timestamp = htole64((monotonic_ns() - start) / 1000);
    size_t patch_size =
        !key_due ? zuse_patch((const uint32_t *)image->data,
                              (const uint32_t *)previous, image->bytes_per_line,
                              &last_shift, patch, PATCH_LIMIT)
                 : 0;
    if (patch_size) {
      // Suppress unchanged frames, retaining the periodic recovery keyframe.
      if (patch_size > 2 || patch[0] || patch[1]) {
        uint32_t raw_length = htole32((uint32_t)patch_size);
        memcpy(compressed, &raw_length, 4);
        int packed =
            LZ4_compress_default((const char *)patch, (char *)compressed + 4,
                                 patch_size, LZ4_compressBound(patch_size));
        if (!packed)
          return 1;
        size_t n = 4 + (size_t)packed;
        uint32_t length = htole32((uint32_t)n | UINT32_C(0x40000000));
        if (fwrite(&length, 4, 1, stdout) != 1 ||
            fwrite(&timestamp, 8, 1, stdout) != 1 ||
            fwrite(compressed, 1, n, stdout) != n || fflush(stdout))
          return 1;
        sent_patch = 1;
      }
    } else {
      input.i_type = sent_patch || key_due ? X264_TYPE_IDR : X264_TYPE_AUTO;
      if (key_due || sent_patch)
        last_key = last_capture;
      sent_patch = 0;
      x264_image_t *planes = &input.img;
      if (ARGBToI420((uint8_t *)image->data, image->bytes_per_line,
                     planes->plane[0], planes->i_stride[0], planes->plane[1],
                     planes->i_stride[1], planes->plane[2], planes->i_stride[2],
                     width, height))
        return 1;
      x264_nal_t *nals;
      int count;
      int size = x264_encoder_encode(encoder, &nals, &count, &input, &output);
      if (size < 0)
        return 1;
      if (size > 0) {
        uint32_t length = htole32(
            (uint32_t)size | (output.b_keyframe ? UINT32_C(0x80000000) : 0));
        if (fwrite(&length, sizeof(length), 1, stdout) != 1 ||
            fwrite(&timestamp, sizeof(timestamp), 1, stdout) != 1 ||
            fwrite(nals[0].p_payload, 1, size, stdout) != (size_t)size ||
            fflush(stdout))
          return 1;
      }
    }
    index ^= 1;
    input.i_pts++;
  }
}
