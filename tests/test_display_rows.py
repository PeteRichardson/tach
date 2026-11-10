"""Host tests for the display rows in code.py (seam: row construction).

Issue #13: Row._y divides by `full`. A None full scale survives construction
and raises TypeError at the first drawn sample — a hard-fault screen on the
device. Row.__init__ must reject it at startup with a clear message instead.
"""
import pytest


def test_base_row_requires_numeric_full_scale(tach_code):
    with pytest.raises(ValueError, match="full scale"):
        tach_code.Row(0, "CPU", 0x40C0FF, None, tach_code.terminalio.FONT, 1, tach_code.fmt_pct)


def test_mem_row_autoscales_and_pushes(tach_code):
    # design.md 5.2: the MEM y axis follows the data, so MemRow ignores its
    # table full_scale. Construction from METRICS must pass the guard, and a
    # push must draw without a fixed scale.
    _, rows = tach_code.build_display(tach_code.terminalio.FONT, 1, True)
    rows["mem"].push(3.2)
    rows["cpu"].push(42.0)
