"""Original Drift icon; stdlib-only build asset generation, no proprietary art."""
import math
import struct
import zlib
from pathlib import Path


def png(size, rounded=True):
    def distance(x, y, a, b):
        dx, dy = b[0] - a[0], b[1] - a[1]
        t = max(0, min(1, ((x - a[0]) * dx + (y - a[1]) * dy) / (dx * dx + dy * dy)))
        return math.hypot(x - a[0] - t * dx, y - a[1] - t * dy)

    rows = []
    for y in range(size):
        row = bytearray([0])
        for x in range(size):
            px, py = (x + .5) / size, (y + .5) / size
            corner = math.hypot(max(.2 - px, 0, px - .8), max(.2 - py, 0, py - .8))
            rgba = (21, 29, 45, 255 if corner <= .2 else 0)
            for center, length in [(.32, .22), (.5, .38), (.68, .22)]:
                if distance(px, py, (center - length * .22, .5 + length / 2), (center + length * .22, .5 - length / 2)) <= .035:
                    rgba = (138, 171, 255, 255)
            row.extend(rgba if rounded else rgba[:3])
        rows.append(row)

    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))
    return b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>2I5B', size, size, 8, 6 if rounded else 2, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(b''.join(rows))) + chunk(b'IEND', b'')


output = Path(__file__).resolve().parent.parent / 'public/icons'
output.mkdir(parents=True, exist_ok=True)
for size in (192, 512):
    (output / f'phone-{size}.png').write_bytes(png(size))
# Source for `npx tauri icon src-tauri/icons/source.png`, which derives the
# macOS, Windows, Linux, Android and iOS icon sets.
app = output.parent.parent / 'src-tauri/icons'
app.mkdir(parents=True, exist_ok=True)
(app / 'source.png').write_bytes(png(1024))
