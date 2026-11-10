"""Host tests for the METRICS table in code.py (seams: build_display, source).

Issue #11: one metric was described in four places that had to agree by hand —
the METRICS tuple, the formatter chosen by display name, the row class chosen
by key, and a hard-coded key tuple in the reader loop. METRICS is now the
single definition: Metric(key, label, color, full_scale, row_class, fmt).
"""
import re


def test_metrics_entry_drives_row_class_and_formatter(tach_code):
    _, rows = tach_code.build_display(tach_code.terminalio.FONT, 1, True)
    for m in tach_code.METRICS:
        row = rows[m.key]
        assert type(row) is m.row_class, f"{m.key}: METRICS row_class not used"
        assert row.fmt is m.fmt, f"{m.key}: METRICS formatter not used"
    # Worked examples of the formatters, independent of how they are chosen.
    assert rows["cpu"].fmt(42.0) == "42%"
    assert rows["mem"].fmt(5.21) == "5.2G"


def test_full_scale_is_always_numeric(tach_code):
    # Row.__init__ rejects None, so no METRICS entry may carry it — including
    # the autoscaling row, which ignores its number.
    for m in tach_code.METRICS:
        assert isinstance(m.full_scale, (int, float)), f"{m.key}: full_scale must be numeric"


def test_parse_loop_keys_come_from_metrics(tach_code, codepy_source):
    # The keys are design.md 5.1's metric set; the parse loop must read them
    # from METRICS, not repeat a hard-coded tuple that can drift from the
    # table. The regex catches any quoting or spacing of the key triple. This
    # asserts on text, not behavior: the loop lives inside main(), which does
    # not return, so it cannot be driven from the host.
    assert tach_code.METRIC_KEYS == ("cpu", "gpu", "mem")
    assert not re.search(r'"cpu"\s*,\s*"gpu"\s*,\s*"mem"', codepy_source)
