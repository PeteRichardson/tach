"""Tests for build_message() in tach-sender.py.

build_message() emits one JSON line the display parses (code.py METRICS keys).
A drift in key names, rounding, separators, or the trailing newline breaks the
pair silently: the display keeps showing the last good values. The receiver
parses by key, so key ORDER is not pinned — what is pinned: the key set, the
rounding per metric, compact separators, and the single trailing newline.
"""
import json


def test_full_message_line_contract(tach_sender):
    """All four metrics: right values, compact separators, one trailing newline."""
    line = tach_sender.build_message(12.5, 37.5, 8.125, 128.0)
    assert json.loads(line) == {"cpu": 12.5, "gpu": 37.5, "mem": 8.12, "mem_total": 128.0}
    # mem rounds to 2 decimals (banker's rounding: 8.125 -> 8.12)
    assert line.endswith("}\n") and line.count("\n") == 1
    assert ", " not in line and '": ' not in line  # compact separators


def test_rounding_precision_by_metric(tach_sender):
    """cpu/gpu round to 1 decimal, mem to 2, mem_total to 1."""
    msg = json.loads(tach_sender.build_message(12.34, 0.05, 8.567, 63.951))
    assert msg == {"cpu": 12.3, "gpu": 0.1, "mem": 8.57, "mem_total": 64.0}


def test_absent_metrics_are_omitted_not_zero(tach_sender):
    """cpu/gpu None: the key must be absent. Zero means idle; absent means
    "no data", and code.py only updates a row for numeric values."""
    msg = json.loads(tach_sender.build_message(None, None, 8.0, 128.0))
    assert msg == {"mem": 8.0, "mem_total": 128.0}


def test_zero_gpu_is_kept(tach_sender):
    """0.0 is a real reading: an idle GPU shows, a missing one is None."""
    msg = json.loads(tach_sender.build_message(1.0, 0.0, 8.0, 128.0))
    assert msg["gpu"] == 0.0


def test_message_keys_cover_display_metrics(tach_sender, tach_code):
    """Wire contract across both sides: every key the display reads
    (code.py METRICS) is present in a full sender message. A rename on one
    side fails here instead of silently freezing the display row."""
    msg = json.loads(tach_sender.build_message(1.0, 2.0, 3.0, 4.0))
    assert set(msg) >= set(tach_code.METRIC_KEYS)
