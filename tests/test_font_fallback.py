"""Host tests for font-failure handling in code.py (seams: load_big_font,
build_display).

Issue #14: load_big_font caught only OSError and ImportError. A truncated or
damaged BDF raises ValueError, KeyError or MemoryError from load_font() or
load_glyphs(); each of those escaped the old handler and crashed the device
into the hard-fault screen. A stopped display is worse than a blocky one, so
any font fault must fall back.

Issue #26: the fallback notice went to the USB console only — a port the
README tells users to keep closed — so the screen must show NO FONT instead.
"""
import sys
from unittest import mock

import pytest
from conftest import bitmap_font_module


def test_any_font_fault_falls_back_to_builtin(tach_code):
    # The parser faults of issue #14 must not escape.
    faults = (ValueError("bad bdf"), KeyError("glyph"), MemoryError(), OSError("io"))
    for exc in faults:
        with mock.patch.dict(sys.modules, bitmap_font_module(exc)):
            font, scale, ok = tach_code.load_big_font()
            assert font is tach_code.terminalio.FONT, f"{exc!r}: no fallback"
            assert scale == 3, f"{exc!r}: wrong built-in scale"
            assert ok is False, f"{exc!r}: fault not reported"


def test_good_font_loads_and_reports_success(tach_code):
    with mock.patch.dict(sys.modules, bitmap_font_module()):
        _font, scale, ok = tach_code.load_big_font()
        assert scale == 1
        assert ok is True


def test_keyboard_interrupt_still_propagates(tach_code):
    # Pin the catch boundary: widening it to BaseException would swallow
    # Ctrl-C and clean exits on the device.
    with mock.patch.dict(sys.modules, bitmap_font_module(KeyboardInterrupt())):
        with pytest.raises(KeyboardInterrupt):
            tach_code.load_big_font()


def _label_texts(group):
    return [c.text for c in group.children if hasattr(c, "text")]


def test_no_font_label_only_on_fallback(tach_code):
    # The notice is screen-global chrome, like the rules, so assert on root.
    root_ok, _ = tach_code.build_display(tach_code.terminalio.FONT, 1, True)
    root_bad, _ = tach_code.build_display(tach_code.terminalio.FONT, 1, False)
    assert "NO FONT" not in _label_texts(root_ok)
    assert "NO FONT" in _label_texts(root_bad)
