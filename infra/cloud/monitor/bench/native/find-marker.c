#include <X11/Xlib.h>
#include <X11/Xutil.h>
#include <stdint.h>
#include <stdio.h>
#include <sys/time.h>
int main(void) {
  Display *d = XOpenDisplay(0);
  if (!d)
    return 1;
  XImage *img =
      XGetImage(d, DefaultRootWindow(d), 0, 0, 1080, 640, AllPlanes, ZPixmap);
  if (!img)
    return 1;
  struct timeval t;
  gettimeofday(&t, 0);
  uint32_t now = (t.tv_sec * 1000ULL + t.tv_usec / 1000) & 0xffffff;
  for (int y = 0; y < 440; y += 2)
    for (int x = 0; x < 500; x += 2) {
      uint64_t marker = 0;
      int b;
      for (b = 0; b < 40; b++) {
        unsigned rgb =
            XGetPixel(img, x + 12 + (b % 20) * 24, y + 12 + (b / 20) * 24) &
            0xffffff;
        if (rgb == 0xffffff)
          marker |= 1ULL << b;
        else if (rgb != 0)
          break;
      }
      if (b == 40 && marker >> 24 &&
          ((now - (uint32_t)marker) & 0xffffff) < 350) {
        printf("%d %d\n", x, y);
        return 0;
      }
    }
  return 2;
}
