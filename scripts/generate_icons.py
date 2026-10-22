"""Original Drift icons; stdlib-only build asset generation, no proprietary art.

Writes src-tauri/icons/source.png (run `npx tauri icon src-tauri/icons/source.png`
afterwards for the app icon set) and src-tauri/icons/tray.png, a monochrome
template image that macOS tints for the light or dark menu bar.
"""
import math
import struct
import zlib
from pathlib import Path

STROKES = [(.32, .22), (.5, .38), (.68, .22)]


def distance(x, y, a, b):
    dx, dy = b[0] - a[0], b[1] - a[1]
    t = max(0, min(1, ((x - a[0]) * dx + (y - a[1]) * dy) / (dx * dx + dy * dy)))
    return math.hypot(x - a[0] - t * dx, y - a[1] - t * dy)


def encode(size, rows):
    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))
    header = struct.pack('>2I5B', size, size, 8, 6, 0, 0, 0)
    return b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', header) + chunk(b'IDAT', zlib.compress(b''.join(rows))) + chunk(b'IEND', b'')


def stroke(px, py, strokes, width):
    return any(distance(px, py, (c - l * .22, .5 + l / 2), (c + l * .22, .5 - l / 2)) <= width for c, l in strokes)


def app_icon(size):
    rows = []
    for y in range(size):
        row = bytearray([0])
        for x in range(size):
            px, py = (x + .5) / size, (y + .5) / size
            corner = math.hypot(max(.2 - px, 0, px - .8), max(.2 - py, 0, py - .8))
            rgba = (21, 29, 45, 255 if corner <= .2 else 0)
            if stroke(px, py, STROKES, .035):
                rgba = (138, 171, 255, 255)
            row.extend(rgba)
        rows.append(row)
    return encode(size, rows)


def tray_icon(size):
    # The same three strokes, larger and black-on-transparent for a template image.
    strokes = [(.22, .36), (.5, .72), (.78, .36)]
    rows = []
    for y in range(size):
        row = bytearray([0])
        for x in range(size):
            px, py = (x + .5) / size, (y + .5) / size
            alpha = max(max(0, min(1, (.075 - distance(px, py, (c - l * .22, .5 + l / 2), (c + l * .22, .5 - l / 2))) * size)) for c, l in strokes)
            row.extend((0, 0, 0, int(alpha * 255)))
        rows.append(row)
    return encode(size, rows)


icons = Path(__file__).resolve().parent.parent / 'src-tauri/icons'
icons.mkdir(parents=True, exist_ok=True)
(icons / 'source.png').write_bytes(app_icon(1024))
(icons / 'tray.png').write_bytes(tray_icon(44))
