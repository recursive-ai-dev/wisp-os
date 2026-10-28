"""Capture and decode Wisp's exact framebuffer font with Python stdlib only.

The console draws 8x16 cells on a black background. Glyph bitmaps come
directly from src/font.rs rather than an OCR engine or a copied font table.
This helper deliberately recognizes only Wisp's console, not firmware text
or arbitrary graphics. Unknown cells are returned as replacement characters.
"""

from functools import lru_cache
import json
from pathlib import Path
import re
import time

ROOT = Path(__file__).resolve().parents[1]
_SPACE = b" \t\r\n\v\f"


def capture(mon, path):
    """Ask a QEMU HMP monitor for a PPM screendump, returning its Path."""
    path = Path(path).resolve()
    if path.suffix.lower() != ".ppm":
        path = path.with_suffix(".ppm")
    path.parent.mkdir(parents=True, exist_ok=True)
    # HMP's double-quoted string syntax accepts JSON's path escaping.
    response = mon.cmd("screendump " + json.dumps(str(path)))
    deadline = time.monotonic() + 5
    while not path.is_file():
        if time.monotonic() >= deadline:
            raise RuntimeError(f"QEMU did not create {path}: {response}")
        time.sleep(0.01)
    return path


@lru_cache(maxsize=1)
def _font():
    source = (ROOT / "src" / "font.rs").read_text(encoding="utf-8")
    first = int(re.search(r"FONT_FIRST:\s*u8\s*=\s*(0x[0-9a-fA-F]+)", source).group(1), 16)
    table = source.split("pub const FONT_DATA", 1)[1].split("=", 1)[1].split("];", 1)[0]
    rows = re.findall(r"^\s*\[([^\]]+)\]\s*,", table, re.MULTILINE)
    glyphs = [bytes(int(value, 16) for value in re.findall(r"0x([0-9a-fA-F]{2})", row)) for row in rows]
    if len(glyphs) != 95 or any(len(glyph) != 16 for glyph in glyphs):
        raise ValueError("src/font.rs does not contain the expected 95 8x16 glyphs")
    exact = {glyph: chr(first + index) for index, glyph in enumerate(glyphs)}
    partial = {}
    for glyph, character in exact.items():
        # Ambiguous truncated glyphs must remain unknown. An underline
        # cursor on a blank cell is the sole special case: prefer space.
        prefix = glyph[:14]
        partial[prefix] = character if prefix not in partial else None
    partial[bytes(14)] = " "
    return exact, partial


def _ppm(path):
    raw = Path(path).read_bytes()
    position = 0

    def skip(index):
        while index < len(raw):
            if raw[index] in _SPACE:
                index += 1
            elif raw[index] == ord("#"):
                while index < len(raw) and raw[index] not in b"\r\n":
                    index += 1
            else:
                break
        return index

    def token():
        nonlocal position
        position = skip(position)
        start = position
        while position < len(raw) and raw[position] not in _SPACE and raw[position] != ord("#"):
            position += 1
        if start == position:
            raise ValueError("truncated PPM header")
        return raw[start:position]

    if token() != b"P6":
        raise ValueError("expected a binary P6 PPM screenshot")
    try:
        width, height, maximum = int(token()), int(token()), int(token())
    except ValueError as error:
        raise ValueError("invalid PPM dimensions/max value") from error
    if width <= 0 or height <= 0 or maximum != 255:
        raise ValueError("PPM requires positive dimensions and max value 255")
    expected = width * height * 3
    raster_start = len(raw) - expected
    # Use the known raster size to preserve a first pixel whose bytes are
    # whitespace or '#', while accepting comments/whitespace in the header.
    if raster_start <= position or raw[position] not in _SPACE:
        raise ValueError("truncated PPM raster or missing header separator")
    header_tail = raw[position:raster_start]
    if re.fullmatch(rb"(?:[ \t\r\n\v\f]+|\#[^\r\n]*(?:\r\n|\r|\n))*", header_tail) is None:
        raise ValueError("PPM raster size does not match its header")
    return width, height, memoryview(raw)[raster_start:]


def text(path):
    """Decode visible console rows; preserve internal spaces and line breaks."""
    width, height, raster = _ppm(path)
    exact, partial = _font()
    lines = []
    stride = width * 3
    for top in range(0, height - 15, 16):
        line = []
        for left in range(0, width - 7, 8):
            glyph = bytearray(16)
            for row in range(16):
                offset = (top + row) * stride + left * 3
                bits = 0
                for column in range(8):
                    pixel = offset + column * 3
                    if raster[pixel] or raster[pixel + 1] or raster[pixel + 2]:
                        bits |= 0x80 >> column
                glyph[row] = bits
            bitmap = bytes(glyph)
            character = exact.get(bitmap)
            if character is None:
                character = partial.get(bitmap[:14])
            line.append(character if character is not None else "\ufffd")
        lines.append("".join(line).rstrip())
    return "\n".join(lines).rstrip()
