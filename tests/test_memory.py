"""Tests for the memory formula in tach-sender.py.

The formula is: (internal - purgeable) + wire + compressor
This matches Activity Monitor's "Memory Used" exactly.

File-backed pages (external_page_count) are deliberately excluded,
matching Activity Monitor. See design.md section 5.1.

The numeric tests build Memory through its real __init__ with ctypes.CDLL and
os.sysconf patched, so the mach-port setup, the argtypes declarations, and the
hw.memsize sysctl are exercised on any machine; the macOS-only test at the end
drives the unpatched path against the live kernel.
"""
import ctypes
import sys
from unittest import mock

import pytest

GB = 1024 ** 3
# Apple Silicon page size; also the worked-example multiply below.
PAGE_SIZE = 16384


def pinned_stats(sender, **values):
    """A real VMStatistics64 with the given fields set (the rest stay 0)."""
    stats = sender.VMStatistics64()
    for name, value in values.items():
        setattr(stats, name, value)
    return stats


def make_memory(sender, stats, kr=0, memsize_gb=256):
    """Memory constructed through its real __init__: libSystem, mach ports and
    sysctl are answered by a fake libc, never the kernel.

    kr != 0 makes host_statistics64 fail like the kernel does.
    """
    libc = mock.Mock()
    libc.mach_host_self.return_value = 42

    def sysctlbyname(name, outp, lenp, newp, newlen):
        assert name == b"hw.memsize"
        # pointer assignment writes through; indexing a simple-type pointer
        # would auto-unbox to int and have nothing to write to
        ctypes.cast(outp, ctypes.POINTER(ctypes.c_uint64))[0] = memsize_gb * GB
        return 0

    libc.sysctlbyname.side_effect = sysctlbyname

    def host_statistics64(host, flavor, stats_ptr, count_ptr):
        # HOST_VM_INFO64 = 4: a wrong flavor means the kernel would return
        # garbage or an error, so the fake says so instead of answering.
        assert flavor == 4, f"unexpected flavor {flavor}"
        if kr:
            return kr
        # stats_ptr is ctypes.byref(...); cast gives a writable pointer to it
        # (byref objects have no .contents on CPython).
        target = ctypes.cast(stats_ptr, ctypes.POINTER(type(stats)))[0]
        for name, _ in type(stats)._fields_:
            setattr(target, name, getattr(stats, name))
        return 0

    libc.host_statistics64.side_effect = host_statistics64
    with mock.patch.object(sender.ctypes, "CDLL", return_value=libc), mock.patch.object(
        sender.os, "sysconf", return_value=PAGE_SIZE
    ):
        return sender.Memory()


def test_used_gb_applies_the_activity_monitor_formula(tach_sender):
    """(internal - purgeable) + wire + compressor, times page size.

    Worked example: (1_000_000 - 100_000) + 500_000 + 200_000 = 1_600_000
    pages x 16 KiB / GiB = 1_600_000 * 16384 / 2**30 = 24.4140625 exactly.
    external/free/active/inactive/speculative/throttled are set large on
    purpose: none may enter the sum. Activity Monitor's "Memory Used"
    excludes file-backed pages; including external_page_count would
    overcount by tens of GB with a large mmap'd model.
    """
    stats = pinned_stats(
        tach_sender,
        internal_page_count=1_000_000,
        purgeable_count=100_000,
        wire_count=500_000,
        compressor_page_count=200_000,
        external_page_count=5_000_000,
        free_count=9_000_000,
        active_count=8_000_000,
        inactive_count=7_000_000,
        speculative_count=6_000_000,
        throttled_count=5_000_000,
    )
    mem = make_memory(tach_sender, stats)
    assert mem.used_gb() == 24.4140625
    assert mem.total_gb == 256.0  # 256 GB through the real sysctl path in __init__


def test_used_gb_raises_when_kernel_call_fails(tach_sender):
    """A non-zero kern_return must raise, not return the zeroed struct's value:
    silent zeros would display 0 GB used forever."""
    mem = make_memory(tach_sender, pinned_stats(tach_sender), kr=4)  # KERN_FAILURE
    with pytest.raises(OSError):
        mem.used_gb()


def test_vm_statistics64_has_external_page_count_field(tach_sender):
    """The struct must carry every field the formula and the excluded-field
    list rely on. external_page_count exists so we could report it, but the
    numeric test above proves it stays out of used_gb()."""
    field_names = [name for name, _ in tach_sender.VMStatistics64._fields_]
    for name in (
        "external_page_count",
        "internal_page_count",
        "wire_count",
        "compressor_page_count",
        "purgeable_count",
    ):
        assert name in field_names, f"{name} missing from VMStatistics64 struct"


def test_vm_statistics64_size_is_pinned(tach_sender):
    """vm_statistics64 on arm64 macOS is 152 bytes: 9 uint64 + 4 uint64 plus
    the uint32 pairs, no trailing padding. Changing a field between c_uint32
    and c_uint64 shifts every later offset and silently misreads the struct.
    Pin the size so the drift is caught here instead."""
    assert ctypes.sizeof(tach_sender.VMStatistics64) == 152


@pytest.mark.skipif(sys.platform != "darwin", reason="macos-only: libSystem and vm pages")
def test_live_used_gb_is_within_total(tach_sender):
    """End-to-end on the real kernel: the ctypes path returns a sane number.

    If the field list ever mismatches the kernel's struct, host_statistics64
    fails (kr != 0) and this raises OSError, instead of shipping wrong readings.
    """
    mem = tach_sender.Memory()
    used = mem.used_gb()
    assert 0.0 < used < mem.total_gb
    assert mem.total_gb >= 8.0
