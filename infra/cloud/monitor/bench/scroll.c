// Reproducible scrolling document. Sixteen bars identify each source frame.
#include <X11/Xlib.h>
#include <stdint.h>
#include <stdio.h>
#include <time.h>
int main(void) {
  Display *d = XOpenDisplay(NULL);
  if (!d)
    return 1;
  int s = DefaultScreen(d);
  XSetWindowAttributes a = {.override_redirect = True};
  Window w =
      XCreateWindow(d, RootWindow(d, s), 0, 0, 1920, 1080, 0, CopyFromParent,
                    InputOutput, CopyFromParent, CWOverrideRedirect, &a);
  XMapRaised(d, w);
  GC gc = XCreateGC(d, w, 0, NULL);
  Pixmap doc = XCreatePixmap(d, w, 1920, 4096, DefaultDepth(d, s));
  XSetForeground(d, gc, 0xf8f8f8);
  XFillRectangle(d, doc, gc, 0, 0, 1920, 4096);
  for (int y = 20; y < 4096; y += 24) {
    char line[256];
    for (int x = 0; x < 250; x++)
      line[x] = 33 + ((y * 37 + x * 13) % 90);
    XSetForeground(d, gc, 0x242824);
    XDrawString(d, doc, gc, 40, y, line, 250);
  }
  struct timespec t;
  clock_gettime(CLOCK_MONOTONIC, &t);
  for (unsigned frame = 1;; frame++) {
    XCopyArea(d, doc, w, gc, 0, (frame * 6) % 3000, 1920, 1080, 0, 0);
    for (int bit = 0; bit < 16; bit++) {
      XSetForeground(d, gc, (frame >> bit) & 1 ? 0xffffff : 0);
      XFillRectangle(d, w, gc, bit * 16, 0, 16, 16);
    }
    XSync(d, False);
    t.tv_nsec += 16666667;
    if (t.tv_nsec >= 1000000000) {
      t.tv_nsec -= 1000000000;
      t.tv_sec++;
    }
    clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, &t, NULL);
  }
}
