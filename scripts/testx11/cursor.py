# Print the X server's current cursor image (XFixesGetCursorImage): size,
# hotspot, a few pixels as ARGB, and whether any pixel's colour is above its
# alpha, to check the native cursor without looking at it. python3 -I cursor.py :42
import ctypes, sys
class Img(ctypes.Structure):
    _fields_ = [("x", ctypes.c_short), ("y", ctypes.c_short),
                ("width", ctypes.c_ushort), ("height", ctypes.c_ushort),
                ("xhot", ctypes.c_ushort), ("yhot", ctypes.c_ushort),
                ("serial", ctypes.c_ulong), ("pixels", ctypes.POINTER(ctypes.c_ulong)),
                ("atom", ctypes.c_ulong), ("name", ctypes.c_char_p)]
x = ctypes.CDLL("libX11.so.6"); xf = ctypes.CDLL("libXfixes.so.3")
x.XOpenDisplay.restype = ctypes.c_void_p
xf.XFixesGetCursorImage.restype = ctypes.POINTER(Img)
xf.XFixesGetCursorImage.argtypes = [ctypes.c_void_p]
d = x.XOpenDisplay(sys.argv[1].encode())
if not d: sys.exit("cannot open display")
i = xf.XFixesGetCursorImage(d).contents
w, h = i.width, i.height
px = [i.pixels[k] & 0xffffffff for k in range(w * h)]
print(f"cursor {w}x{h} hotspot {i.xhot},{i.yhot} at {i.x},{i.y} name {i.name}")
for (cx, cy) in [(0, 0), (w // 2, h // 2), (w - 1, h - 1)]:
    print(f"  pixel {cx},{cy} = {px[cy * w + cx]:#010x}")
# A Render cursor's pixels are premultiplied: no colour channel above alpha.
bad = sum(1 for p in px if max((p >> 16) & 255, (p >> 8) & 255, p & 255) > p >> 24)
partial = sum(1 for p in px if 0 < p >> 24 < 255)
print(f"  {partial} partly transparent pixels; {bad} with a colour channel above alpha")
