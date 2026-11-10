"""Host fixtures for code.py, the CircuitPython program.

code.py imports device modules (board, displayio, usb_cdc, ...) that do not
exist on the host, and it calls main() at import time, which never returns.
The tach_code fixture injects minimal fakes for the device modules and runs
code.py with its trailing main() call stripped, so tests can construct the
classes and call build_display() directly.

The module name is tach_code, not code, so it cannot shadow the stdlib `code`
module; the tach_sender loader below names its module tach_sender for the same
reason.
"""
import json
import os
import sys
import types
from unittest import mock

import pytest

CODE_PATH = os.path.join(os.path.dirname(__file__), "..", "code.py")


class FakeBitmap:
    def __init__(self, width, height, colors):
        self.width = width
        self.height = height
        self.colors = colors

    def __setitem__(self, key, value):
        pass


class FakePalette:
    def __init__(self, capacity):
        self.capacity = capacity

    def __setitem__(self, key, value):
        pass


class FakeGroup:
    def __init__(self, **kwargs):
        self.kwargs = kwargs
        self.children = []

    def append(self, child):
        self.children.append(child)


class FakeTileGrid:
    def __init__(self, bitmap, pixel_shader=None, **kwargs):
        self.bitmap = bitmap
        self.kwargs = kwargs

    def __setitem__(self, key, value):
        pass


class FakeLabel:
    def __init__(self, font, text="", color=None, scale=1):
        self.text = text
        self.color = color
        self.scale = scale


def _fake_modules():
    displayio = types.ModuleType("displayio")
    displayio.Group = FakeGroup
    displayio.Bitmap = FakeBitmap
    displayio.Palette = FakePalette
    displayio.TileGrid = FakeTileGrid

    terminalio = types.ModuleType("terminalio")
    terminalio.FONT = object()

    board = types.ModuleType("board")
    board.DISPLAY = object()

    usb_cdc = types.ModuleType("usb_cdc")
    usb_cdc.data = None

    bitmaptools = types.ModuleType("bitmaptools")
    bitmaptools.fill_region = lambda *args, **kwargs: None

    adafruit_display_text = types.ModuleType("adafruit_display_text")
    label_mod = types.ModuleType("adafruit_display_text.label")
    label_mod.Label = FakeLabel
    adafruit_display_text.label = label_mod

    return {
        "displayio": displayio,
        "terminalio": terminalio,
        "board": board,
        "usb_cdc": usb_cdc,
        "bitmaptools": bitmaptools,
        "adafruit_display_text": adafruit_display_text,
        "adafruit_display_text.label": label_mod,
    }


def bitmap_font_module(exc=None):
    """Fake adafruit_bitmap_font package; load_font raises exc when given.

    code.py imports this inside load_big_font(), so tests patch sys.modules
    at call time; the tach_code fixture only fakes what the import needs.
    """
    pkg = types.ModuleType("adafruit_bitmap_font")
    mod = types.ModuleType("adafruit_bitmap_font.bitmap_font")
    pkg.bitmap_font = mod
    if exc is None:

        class GoodFont:
            def load_glyphs(self, chars):
                pass

        mod.load_font = lambda path: GoodFont()
    else:

        def load_font(path):
            raise exc

        mod.load_font = load_font
    return {
        "adafruit_bitmap_font": pkg,
        "adafruit_bitmap_font.bitmap_font": mod,
    }


@pytest.fixture(scope="session")
def tach_sender():
    """tach-sender.py as a module, despite its hyphenated filename."""
    import importlib.util

    path = os.path.join(os.path.dirname(__file__), "..", "tach-sender.py")
    spec = importlib.util.spec_from_file_location("tach_sender", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@pytest.fixture(scope="session")
def macmon_lines():
    """Parsed dicts from tests/fixtures/macmon_pipe.jsonl (recorded `macmon pipe`)."""
    path = os.path.join(os.path.dirname(__file__), "fixtures", "macmon_pipe.jsonl")
    with open(path) as f:
        return [json.loads(line) for line in f if line.strip()]


@pytest.fixture(scope="session")
def codepy_source():
    with open(CODE_PATH) as f:
        return f.read()


@pytest.fixture(scope="session")
def tach_code(codepy_source):
    stripped = codepy_source.rstrip()
    assert stripped.endswith("main()"), (
        "code.py no longer ends with main(); update this loader"
    )
    src = stripped[: -len("main()")]

    module = types.ModuleType("tach_code")
    fakes = _fake_modules()
    # patch.dict restores sys.modules exactly, so no fake device module leaks
    # into tests that run later.
    with mock.patch.dict(sys.modules, fakes):
        exec(compile(src, CODE_PATH, "exec"), module.__dict__)
    return module
