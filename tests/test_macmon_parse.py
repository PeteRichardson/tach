"""Tests for cpu_gpu_from_macmon() in tach-sender.py.

macmon changes its JSON shape between versions, and a wrong field read fails
silently: the display just shows wrong numbers. These tests pin both shapes:

- current macmon: top-level `cpu_active_ratio` / `gpu_active_ratio` (0-1 ratios)
- macmon 0.x: `ecpu_usage` / `pcpu_usage` / `gpu_usage` as [freq_mhz, ratio]

fixtures/macmon_pipe.jsonl holds three lines recorded from
`macmon pipe -i 500 -s 3` on an Apple Silicon Mac (its UTC timestamps read
2026-11-10), with the per-core arrays (cpu_tiers, ecpu_cores, pcpu_cores)
pruned; every top-level field is as recorded.

Expected values are derived here in the test from the recorded fields, using
the conversion each macmon version documents (ratio x100; the legacy arrays
average to a percent) — the derivation is the spec, the fixture is the data.
"""
import copy

import pytest


def legacy_cpu_pct(line):
    """The macmon 0.x contract: mean of the E-core and P-core ratios, in %."""
    return (line["ecpu_usage"][1] + line["pcpu_usage"][1]) / 2 * 100


def test_recorded_lines_parse_to_active_percentages(tach_sender, macmon_lines):
    """Every recorded line parses, in 0-100, to its own recorded ratios x100."""
    assert len(macmon_lines) >= 3
    for line in macmon_lines:
        cpu, gpu = tach_sender.cpu_gpu_from_macmon(line)
        assert cpu == pytest.approx(line["cpu_active_ratio"] * 100)
        assert gpu == pytest.approx(line["gpu_active_ratio"] * 100)
        assert 0.0 <= cpu <= 100.0
        assert 0.0 <= gpu <= 100.0


def test_current_ratios_win_over_legacy_arrays(tach_sender, macmon_lines):
    """A recorded line carries both shapes; the *_active_ratio fields must win.

    On the recording the legacy arrays give a different answer
    (0.0173 + 0.0121 / 2 vs 0.0347), so the cross-check below fails if the
    branch order or the field names drift.
    """
    line = macmon_lines[0]
    cpu, gpu = tach_sender.cpu_gpu_from_macmon(line)
    assert cpu == pytest.approx(line["cpu_active_ratio"] * 100, abs=1e-9)
    assert gpu == pytest.approx(line["gpu_active_ratio"] * 100, abs=1e-9)
    assert abs(cpu - legacy_cpu_pct(line)) > 0.5
    assert abs(gpu - line["gpu_usage"][1] * 100) > 0.5


def test_legacy_line_without_ratios(tach_sender, macmon_lines):
    """A macmon 0.x line (no *_active_ratio) falls back to the usage arrays:
    cpu = (ecpu + pcpu) / 2, gpu = gpu_usage, both x100."""
    legacy = copy.deepcopy(macmon_lines[0])
    legacy.pop("cpu_active_ratio")
    legacy.pop("gpu_active_ratio")
    cpu, gpu = tach_sender.cpu_gpu_from_macmon(legacy)
    assert cpu == pytest.approx(legacy_cpu_pct(legacy), abs=1e-9)
    assert gpu == pytest.approx(legacy["gpu_usage"][1] * 100, abs=1e-9)


def test_missing_gpu_gives_none_gpu(tach_sender, macmon_lines):
    """No gpu fields at all: cpu still parses, gpu is None."""
    no_gpu = copy.deepcopy(macmon_lines[0])
    no_gpu.pop("gpu_active_ratio")
    no_gpu.pop("gpu_usage")
    cpu, gpu = tach_sender.cpu_gpu_from_macmon(no_gpu)
    assert cpu == pytest.approx(no_gpu["cpu_active_ratio"] * 100, abs=1e-9)
    assert gpu is None


def test_empty_object_gives_none_both(tach_sender):
    """An object with no recognized fields returns (None, None), not 0:
    a missing metric must not look like an idle machine."""
    assert tach_sender.cpu_gpu_from_macmon({}) == (None, None)
