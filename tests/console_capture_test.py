#!/usr/bin/env python3
"""Run with python3 tests/console_capture_test.py; no imaging dependencies."""

from pathlib import Path
import tempfile
import unittest

import console_capture


class ConsoleCaptureTests(unittest.TestCase):
    def test_all_printable_glyphs_and_cursor(self):
        exact, _ = console_capture._font()
        by_character = {character: glyph for glyph, character in exact.items()}
        line = "".join(chr(value) for value in range(32, 127))
        width, height = len(line) * 8, 32
        raster = bytearray(width * height * 3)
        for row, content in enumerate((line, "save: hello _ cursor")):
            for column, character in enumerate(content):
                for y, bits in enumerate(by_character[character]):
                    for x in range(8):
                        if bits & (0x80 >> x):
                            offset = ((row * 16 + y) * width + column * 8 + x) * 3
                            raster[offset:offset + 3] = bytes((20, 200, 35))
        # A two-row cursor underline over an otherwise blank cell.
        cursor_column = len("save: hello _ cursor")
        for y in (30, 31):
            for x in range(8):
                offset = (y * width + cursor_column * 8 + x) * 3
                raster[offset:offset + 3] = b"\xff\x55\x55"
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "screen.ppm"
            path.write_bytes(f"P6\n# comment\n{width}\t{height}\n255\r\n".encode() + raster)
            self.assertEqual(console_capture.text(path), line + "\nsave: hello _ cursor")

    def test_raster_bytes_are_not_consumed_as_header_whitespace(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "screen.ppm"
            payload = b"\x20\x0a\x23"
            path.write_bytes(b"P6\n1 1\n255\n" + payload)
            width, height, raster = console_capture._ppm(path)
            self.assertEqual((width, height, bytes(raster)), (1, 1, payload))
            path.write_bytes(b"P6\n8 16\n255\nshort")
            with self.assertRaises(ValueError):
                console_capture.text(path)


if __name__ == "__main__":
    unittest.main()
