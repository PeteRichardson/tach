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


def test_label_upper_left_number_below_it(tach_code):
    """The label sits in the upper-left corner at twice the built-in size;
    the big number is left-aligned under it, not beside it."""
    _, rows = tach_code.build_display(tach_code.terminalio.FONT, 1, True)
    cpu = rows["cpu"]
    name, number = cpu.group.children[0], cpu.number
    assert name.scale == tach_code.LABEL_SCALE
    assert name.anchor_point == (0.0, 0.0)
    assert name.anchored_position == (tach_code.TEXT_X, tach_code.LABEL_Y)
    assert number.anchor_point == (1.0, 0.0)
    assert number.anchored_position[0] == tach_code.NUMBER_RIGHT_X
    # below the label, inside the row, with a visible gap (>= 5 px of black
    # between the label line and the number top)
    assert number.anchored_position[1] >= tach_code.LABEL_Y + tach_code.LABEL_LINE_H * tach_code.LABEL_SCALE + 5
    assert number.anchored_position[1] < tach_code.ROW_H


def test_numbers_right_align_in_one_column(tach_code):
    """All three numbers, and their bold copies, hang from the same static x:
    the % and G units line up, and the value width changes nothing."""
    _, rows = tach_code.build_display(tach_code.terminalio.FONT, 1, True)
    for key, row in rows.items():
        assert row.number.anchored_position[0] == tach_code.NUMBER_RIGHT_X, key
        assert row.number_bold.anchored_position[0] == tach_code.NUMBER_RIGHT_X + tach_code.BOLD_SHIFT, key
        # the units column stops short of the chart (axis labels share the
        # column vertically: they sit above and below the number)
        assert tach_code.NUMBER_RIGHT_X + tach_code.BOLD_SHIFT + 1 <= tach_code.CHART_X, key
    # worst-case strings ("100%", "97.4G") are ~94 px wide: the right edge
    # must stay at or right of 94 so they are not clipped at the screen edge
    assert tach_code.NUMBER_RIGHT_X >= 94


def test_big_numbers_are_double_drawn_bold(tach_code):
    """Fake bold: a second label with the same text, one pixel right, the same
    color and baseline; show() keeps text and stale color in sync."""
    _, rows = tach_code.build_display(tach_code.terminalio.FONT, 1, True)
    cpu = rows["cpu"]
    edge = cpu.group.children[-1]  # appended last: z-order above the row
    assert edge.anchored_position[0] == cpu.number.anchored_position[0] + tach_code.BOLD_SHIFT
    assert edge.anchored_position[1] == cpu.number.anchored_position[1]
    cpu.show(78.0, False)
    assert edge.text == cpu.number.text == "78%"
    assert edge.color == cpu.number.color == cpu.color
    cpu.show(None, True)
    assert edge.color == tach_code.STALE_COLOR


def test_no_text_measurement_api(codepy_source):
    """Regression pin: the device build's Label has no `dimensions`
    (AttributeError on device, 2026-11-09), and the property's name changes
    across adafruit_display_text versions. Text metrics come from constants."""
    assert "dimensions" not in codepy_source


def test_charts_fill_the_rest_of_the_row(tach_code):
    """The chart starts after the number slot and runs to the right screen
    edge: ~3/4 of the width, the widest it gets without crowding the text."""
    c = tach_code
    assert c.CHART_X == c.TEXT_X + c.NUMBER_W + c.CHART_GAP
    assert c.CHART_W == c.SCREEN_W - c.CHART_X
    assert c.CHART_W * 5 >= c.SCREEN_W * 3  # at least 3/5 of the screen width
    assert c.CHART_W > 170  # wider than the v0.1 chart


def test_mem_axis_labels_left_of_chart(tach_code):
    """The MEM y-axis limits are right-aligned to the left of the chart,
    outside the plot area, aligned with its top and bottom edges."""
    _, rows = tach_code.build_display(tach_code.terminalio.FONT, 1, True)
    mem = rows["mem"]
    # children: 0 label, 1 number, 2 chart grid, 3 top limit, 4 bottom limit
    top, bottom = mem.group.children[3], mem.group.children[4]
    x = tach_code.CHART_X - tach_code.AXIS_GAP
    assert top.anchor_point == (1.0, 0.0)
    assert top.anchored_position == (x, tach_code.CHART_Y)
    assert bottom.anchor_point == (1.0, 1.0)
    assert bottom.anchored_position == (x, tach_code.ROW_H - 2)
    # inside the row: the bottom label stays above the row rule
    assert bottom.anchored_position[1] < tach_code.ROW_H
    # clears the big number: number bottom edge is at or above label top edge
    number_bottom = tach_code.NUMBER_Y + 36  # 36 px big font
    assert number_bottom <= bottom.anchored_position[1] - 11 + 1
