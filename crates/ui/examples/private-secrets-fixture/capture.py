"""Capture only this fixture's X11 client window; never capture the desktop."""
import ctypes as c
import sys
from PIL import Image
x = c.CDLL("libX11.so.6")
D, W = c.c_void_p, c.c_ulong
x.XOpenDisplay.argtypes = [c.c_char_p]; x.XOpenDisplay.restype = D
x.XDefaultRootWindow.argtypes = [D]; x.XDefaultRootWindow.restype = W
x.XInternAtom.argtypes = [D,c.c_char_p,c.c_int]; x.XInternAtom.restype = W
x.XQueryTree.argtypes = [D,W,c.POINTER(W),c.POINTER(W),c.POINTER(c.POINTER(W)),c.POINTER(c.c_uint)]
x.XGetWindowProperty.argtypes = [D,W,W,c.c_long,c.c_long,c.c_int,W,c.POINTER(W),c.POINTER(c.c_int),c.POINTER(W),c.POINTER(W),c.POINTER(c.c_void_p)]
x.XFree.argtypes = [c.c_void_p]
x.XGetGeometry.argtypes = [D,W,c.POINTER(W),c.POINTER(c.c_int),c.POINTER(c.c_int),c.POINTER(c.c_uint),c.POINTER(c.c_uint),c.POINTER(c.c_uint),c.POINTER(c.c_uint)]
class XImage(c.Structure):
    _fields_ = [("width",c.c_int),("height",c.c_int),("xoffset",c.c_int),("format",c.c_int),("data",D),("byte_order",c.c_int),("bitmap_unit",c.c_int),("bitmap_bit_order",c.c_int),("bitmap_pad",c.c_int),("depth",c.c_int),("bytes_per_line",c.c_int),("bits_per_pixel",c.c_int),("red_mask",W),("green_mask",W),("blue_mask",W)]
x.XGetImage.argtypes=[D,W,c.c_int,c.c_int,c.c_uint,c.c_uint,W,c.c_int]; x.XGetImage.restype=c.POINTER(XImage)
x.XDestroyImage.argtypes=[c.POINTER(XImage)]
x.XCloseDisplay.argtypes=[D]
d = x.XOpenDisplay(None)
assert d, "X11 display unavailable"
atom = x.XInternAtom(d,b"_NET_WM_PID",1)
pid = int(sys.argv[1])
def find(window):
    actual,fmt,n,left,data=W(),c.c_int(),W(),W(),D()
    x.XGetWindowProperty(d,window,atom,0,1,0,0,c.byref(actual),c.byref(fmt),c.byref(n),c.byref(left),c.byref(data))
    match = bool(data and n.value and fmt.value == 32 and c.cast(data,c.POINTER(W))[0] == pid)
    if data: x.XFree(data)
    if match: return window
    root,parent,children,count=W(),W(),c.POINTER(W)(),c.c_uint()
    x.XQueryTree(d,window,c.byref(root),c.byref(parent),c.byref(children),c.byref(count))
    ids = [children[i] for i in range(count.value)]
    if children: x.XFree(children)
    for child in ids:
        result = find(child)
        if result: return result
window = find(x.XDefaultRootWindow(d))
assert window, "fixture window not found"
root,xx,yy,width,height,border,depth=W(),c.c_int(),c.c_int(),c.c_uint(),c.c_uint(),c.c_uint(),c.c_uint()
x.XGetGeometry(d,window,c.byref(root),c.byref(xx),c.byref(yy),c.byref(width),c.byref(height),c.byref(border),c.byref(depth))
image = x.XGetImage(d,window,0,0,width.value,height.value,W(-1),2)
assert image and image.contents.bits_per_pixel == 32 and image.contents.byte_order == 0, "unsupported X11 pixel format"
buffer = c.string_at(image.contents.data,image.contents.bytes_per_line*height.value)
Image.frombytes("RGB",(width.value,height.value),buffer,"raw","BGRX",image.contents.bytes_per_line).save(sys.argv[2])
x.XDestroyImage(image); x.XCloseDisplay(d)
