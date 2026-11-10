"""Tests for the MEM chart math in code.py: fmt_gb, nice_range, log_edges.

These three compute what the user reads off the screen: the number label, the
y axis bounds, and the compressed time axis. A wrong value fails silently —
the chart just looks odd, nothing crashes. fmt_gb and nice_range are pinned to
exact expected values (nice_range picks steps of 0.5 and 10 exactly, the 0.2
steps get pytest.approx); log_edges solves b by bisection, so it is pinned by
properties instead. The chart geometry comes from code.py's own constants, so
retuning CHART_W or MEM_HISTORY_S moves the tests with the product.
"""
import pytest

# --- fmt_gb --------------------------------------------------------------------


def test_fmt_gb_two_digit_and_three_digit_forms(tach_code):
    assert tach_code.fmt_gb(97.4) == "97.4G"
    assert tach_code.fmt_gb(128) == "128G"
    assert tach_code.fmt_gb(0.0) == "0.0G"


def test_fmt_gb_boundary_at_99_95(tach_code):
    """Below 99.95 shows one decimal; at or above, whole gigabytes —
    so the string never exceeds 5 characters ("99.9G" vs "100G")."""
    assert tach_code.fmt_gb(99.94) == "99.9G"
    assert tach_code.fmt_gb(99.95) == "100G"


def test_fmt_gb_never_exceeds_five_chars(tach_code):
    for value in (0.0, 0.04, 8.12, 97.4, 99.94, 99.95, 100.0, 128.0, 256.0, 999.9):
        text = tach_code.fmt_gb(value)
        assert len(text) <= 5, f"fmt_gb({value}) = {text!r} overflows the 5-char slot"


# --- nice_range ---------------------------------------------------------------


# (lo, hi) in, (lo, hi) out. Steps of 0.5, 1 and 10 are binary-exact; the
# 0.2-step cases keep ~1e-15 float noise, hence approx everywhere.
NICE_CASES = [
    # span 1.8 -> step 0.5: floor(7.3/0.5)*0.5 = 7.0, ceil(9.1/0.5)*0.5 = 9.5
    ((7.3, 9.1), (7.0, 9.5)),
    # span 0.2 -> widened to span 1.0 around 8.1 -> step 0.2 -> 7.4 .. 8.6
    ((8.0, 8.2), (7.4, 8.6)),
    # widened lo -0.45 clamps to 0, so the min span relaxes to 0.6 here.
    # Unreachable live: macOS at idle uses far more than 0.5 GB.
    ((0.0, 0.1), (0.0, 0.6)),
    # span 49.5 -> step 10 -> 10 .. 70
    ((12.4, 61.9), (10.0, 70.0)),
    # span 27.5 -> step 10 -> 100 .. 130
    ((100.0, 127.5), (100.0, 130.0)),
    # negative lo clamps to 0; step 0.5 -> 0 .. 0.5
    ((-2.0, 0.3), (0.0, 0.5)),
]


@pytest.mark.parametrize(("lo_hi", "expected"), NICE_CASES)
def test_nice_range(tach_code, lo_hi, expected):
    """The axis covers the input, never goes negative, and moves to whole
    steps so it does not shift at every tick."""
    assert tach_code.nice_range(*lo_hi) == pytest.approx(expected)


def test_nice_range_span_is_never_zero(tach_code):
    """_redraw divides k by (hi - lo): a zero span would crash the device.
    Away from the clamp at zero the span also stays at or above the minimum
    (5.0, 5.0) widens to 4.4..5.6; at zero it relaxes to 0..0.6, same
    documented exception as the (0.0, 0.1) case above."""
    out_lo, out_hi = tach_code.nice_range(5.0, 5.0)
    assert out_hi - out_lo >= tach_code.MEM_MIN_SPAN_GB - 1e-9
    out_lo, out_hi = tach_code.nice_range(0.0, 0.0)
    assert 0.0 <= out_lo < out_hi


# --- log_edges ----------------------------------------------------------------


@pytest.fixture(scope="module")
def chart_axes(tach_code):
    """The real MEM chart geometry: columns = CHART_W, samples = history
    window / tick, both derived from code.py so retuning moves the tests."""
    columns = tach_code.CHART_W
    samples = int(tach_code.MEM_HISTORY_S * 1e9 / tach_code.TICK_NS)
    return columns, samples, tach_code.log_edges(columns, samples)


def test_log_edges_shape(chart_axes):
    """columns + 1 edges, starting at 0, strictly increasing by at least 1."""
    columns, samples, edges = chart_axes
    assert samples == 600  # 5 min at 2 Hz: sanity on the derivation
    assert len(edges) == columns + 1
    assert edges[0] == 0
    for a, b in zip(edges, edges[1:], strict=False):  # edges[1:] is one shorter
        assert b - a >= 1


def test_log_edges_covers_all_samples_without_waste(chart_axes):
    _, samples, edges = chart_axes
    assert samples <= edges[-1] <= samples + 1


def test_log_edges_one_sample_per_column_at_the_right_edge(chart_axes):
    """Age starts at exactly one sample per column near "now". Depth 8 is a
    checked margin: column 9 already skips ahead (edge 10) on this geometry."""
    _, _, edges = chart_axes
    assert edges[:8] == list(range(8))


def test_log_edges_compresses_toward_the_left_edge(chart_axes):
    """The window per column grows with age: mean gap of the newest columns
    is smaller than the mean gap of the oldest."""
    columns, _, edges = chart_axes
    newest = (edges[10] - edges[0]) / 10
    oldest = (edges[columns] - edges[columns - 10]) / 10
    assert newest < oldest


def test_log_edges_samples_below_columns_floors_at_columns(tach_code):
    """Fewer samples than columns: each column still gets at least one
    sample-slot (samples floored up to columns)."""
    edges = tach_code.log_edges(tach_code.CHART_W, tach_code.CHART_W // 2)
    assert len(edges) == tach_code.CHART_W + 1
    assert edges[-1] >= tach_code.CHART_W
