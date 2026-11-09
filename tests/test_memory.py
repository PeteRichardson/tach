"""Tests for the memory formula in tach-sender.py.

The formula is: (internal - purgeable) + wire + compressor
This matches Activity Monitor's "Memory Used" exactly.

File-backed pages (external_page_count) are deliberately excluded,
matching Activity Monitor. See design.md section 5.1.
"""
import importlib.util
import os
from unittest import mock


def load_tach_sender():
    """Load tach-sender.py as a module despite its hyphenated filename."""
    sender_path = os.path.join(os.path.dirname(__file__), "..", "tach-sender.py")
    spec = importlib.util.spec_from_file_location("tach_sender", sender_path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class FakeStats:
    """Fake vm_statistics64 struct values for testing."""
    def __init__(self, **kwargs):
        for k, v in kwargs.items():
            setattr(self, k, v)


def test_used_gb_excludes_external_page_count():
    """used_gb() must NOT include external_page_count (file-backed/mmap pages).

    Activity Monitor's "Memory Used" excludes file-backed pages.
    Adding them would overcount by ~80 GB on a 256 GB machine.
    """
    sender = load_tach_sender()

    # Simulate vm_statistics64 with external pages present
    stats = FakeStats(
        internal_page_count=1000,
        purgeable_count=100,
        wire_count=500,
        compressor_page_count=200,
        external_page_count=10000,  # large mmap'd model
    )

    page_size = 16384
    GB = 1024 ** 3

    # The formula should be: (internal - purgeable) + wire + compressor
    # NOT: + external_page_count
    app = stats.internal_page_count - stats.purgeable_count
    pages_without_external = app + stats.wire_count + stats.compressor_page_count
    pages_with_external = pages_without_external + stats.external_page_count

    gb_without_external = pages_without_external * page_size / GB
    gb_with_external = pages_with_external * page_size / GB

    # Verify external pages would add a significant amount
    assert gb_with_external > gb_without_external, (
        "external_page_count should contribute memory if included"
    )

    # Verify the formula used in used_gb() does NOT include external
    # by checking the actual code doesn't reference external_page_count
    import inspect
    source = inspect.getsource(sender.Memory.used_gb)
    assert "external_page_count" not in source, (
        "used_gb() must not include external_page_count in its formula. "
        "Activity Monitor excludes file-backed pages from Memory Used."
    )


def test_vm_statistics64_has_external_page_count_field():
    """The VMStatistics64 struct should have external_page_count field.

    It exists so we could optionally report it, but it must NOT
    be in the used_gb() formula.
    """
    sender = load_tach_sender()
    field_names = [name for name, _ in sender.VMStatistics64._fields_]
    assert "external_page_count" in field_names, (
        "external_page_count field missing from VMStatistics64 struct"
    )
    assert "internal_page_count" in field_names
    assert "wire_count" in field_names
    assert "compressor_page_count" in field_names
    assert "purgeable_count" in field_names