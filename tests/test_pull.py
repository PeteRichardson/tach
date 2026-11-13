"""Tests for the v2 pull path in tach-sender.py: the request parser, the reply
line, the memory window, CPU from Mach ticks, and finding the v2 device.

Wire format: docs/device-v2.md, "Wire". The device sends {"pull":N}; the reply
echoes "pull":N so the device can drop a late reply to an earlier pull.
"""
import ctypes
import json
from unittest import mock

from test_port_detection import FakePort

Q36_KEYS = {"pull", "v", "t", "hz", "cpu", "tc", "gpu", "tg", "mem", "mem_lo", "mem_hi", "tm", "mem_total", "ok"}


# --- parse_pull ---------------------------------------------------------------

def test_parse_pull_reads_the_sequence_number(tach_sender):
    assert tach_sender.parse_pull(b'{"pull":42}') == 42


def test_parse_pull_rejects_lines_that_are_not_pulls(tach_sender):
    for line in [b"", b"garbage", b'{"pul":1}', b'{"pull":"1"}', b'{"pull":true}', b"[1]", b'{"pull":1.5}']:
        assert tach_sender.parse_pull(line) is None, line


# --- build_reply --------------------------------------------------------------

def reply(sender, **over):
    args = dict(seq=7, t=1000, cpu=12.34, tc=999, gpu_sample=(45.67, 950),
                mem_window=(61.234, 60.9, 61.456, 990), mem_total=128.0)
    args.update(over)
    line = sender.build_reply(**args)
    assert line.endswith("\n") and line.count("\n") == 1
    return json.loads(line)


def test_full_reply_carries_every_q36_key(tach_sender):
    msg = reply(tach_sender)
    assert set(msg) == Q36_KEYS
    assert msg["pull"] == 7
    assert msg["v"] == tach_sender.PROTOCOL_VERSION
    assert (msg["t"], msg["tc"], msg["tg"], msg["tm"]) == (1000, 999, 950, 990)
    assert (msg["cpu"], msg["gpu"]) == (12.3, 45.7)
    assert (msg["mem"], msg["mem_lo"], msg["mem_hi"], msg["mem_total"]) == (61.23, 60.9, 61.46, 128.0)
    assert msg["ok"] is True


def test_pull_is_the_first_key(tach_sender):
    """The device finds "pull":N with a byte search; first is easiest to read in a log."""
    line = tach_sender.build_reply(3, 0, None, None, None, (None, None, None, None), 1.0)
    assert line.startswith('{"pull":3,')


def test_stale_macmon_drops_gpu_and_clears_ok(tach_sender):
    msg = reply(tach_sender, gpu_sample=None)
    assert "gpu" not in msg and "tg" not in msg
    assert msg["ok"] is False
    assert "cpu" in msg and "mem" in msg


def test_unknown_values_are_omitted_not_zero(tach_sender):
    msg = reply(tach_sender, cpu=None, mem_window=(None, None, None, None))
    for key in ("cpu", "tc", "mem", "mem_lo", "mem_hi", "tm"):
        assert key not in msg


def test_full_reply_fits_the_device_line_buffer(tach_sender):
    """device/src/link.rs drops a line longer than LINE_MAX (256 B)."""
    line = tach_sender.build_reply(4294967295, 10**12, 100.0, 10**12, (100.0, 10**12),
                                   (1023.99, 1023.99, 1023.99, 10**12), 1024.0)
    assert len(line.encode()) - 1 <= 256


# --- MemWindow ----------------------------------------------------------------

def test_mem_window_keeps_low_and_high_since_last_take(tach_sender):
    w = tach_sender.MemWindow()
    for gb, t in [(5.0, 1), (7.0, 2), (6.0, 3)]:
        w.add(gb, t)
    assert w.take() == (6.0, 5.0, 7.0, 3)
    # The next window starts at the newest read, not empty.
    assert w.take() == (6.0, 6.0, 6.0, 3)
    w.add(9.0, 4)
    assert w.take() == (9.0, 6.0, 9.0, 4)


def test_mem_window_empty_before_first_read(tach_sender):
    assert tach_sender.MemWindow().take() == (None, None, None, None)


# --- CpuTicks -----------------------------------------------------------------

def make_cpu_ticks(sender, samples):
    """CpuTicks through its real __init__, answered by a fake libc.

    samples: one entry per host_processor_info call; each entry is a list of
    (user, system, idle, nice) tuples, one per core.
    """
    libc = mock.Mock()
    libc.mach_host_self.return_value = 42
    libc.mach_task_self.return_value = 43
    feed = iter(samples)
    buffers = []  # keep each buffer alive until the test ends

    def host_processor_info(host, flavor, cores_ptr, info_ptr, count_ptr):
        assert flavor == 2, f"unexpected flavor {flavor}"  # PROCESSOR_CPU_LOAD_INFO
        cores = next(feed)
        flat = (ctypes.c_uint32 * (4 * len(cores)))(*[v for core in cores for v in core])
        buffers.append(flat)
        ctypes.cast(cores_ptr, ctypes.POINTER(ctypes.c_uint32))[0] = len(cores)
        ctypes.cast(info_ptr, ctypes.POINTER(ctypes.POINTER(ctypes.c_uint32)))[0] = ctypes.cast(
            flat, ctypes.POINTER(ctypes.c_uint32))
        ctypes.cast(count_ptr, ctypes.POINTER(ctypes.c_uint32))[0] = len(flat)
        return 0

    libc.host_processor_info.side_effect = host_processor_info
    with mock.patch.object(sender.ctypes, "CDLL", return_value=libc):
        cpu = sender.CpuTicks()
    cpu.vm_deallocate_calls = libc.vm_deallocate.call_args_list
    return cpu


def test_cpu_is_busy_over_busy_plus_idle_across_all_cores(tach_sender):
    # busy = user + system + nice
    cpu = make_cpu_ticks(tach_sender, [
        [(100, 50, 1000, 0), (0, 0, 0, 0)],
        [(130, 60, 1060, 10), (0, 0, 40, 0)],
    ])
    assert cpu.read() is None  # nothing to compare with yet
    # core 0: d_busy = 30 + 10 + 10 = 50, d_idle = 60; core 1: d_idle = 40
    assert abs(cpu.read() - 100 * 50 / 150) < 1e-9


def test_cpu_frees_the_kernel_buffer_on_every_read(tach_sender):
    cpu = make_cpu_ticks(tach_sender, [[(0, 0, 0, 0)] * 3, [(1, 0, 1, 0)] * 3])
    cpu.read()
    cpu.read()
    assert len(cpu.vm_deallocate_calls) == 2
    task, _addr, size = cpu.vm_deallocate_calls[0].args
    assert (task, size) == (43, 3 * 4 * 4)


def test_cpu_keeps_last_value_when_no_tick_passed(tach_sender):
    cpu = make_cpu_ticks(tach_sender, [[(0, 0, 0, 0)], [(10, 0, 30, 0)], [(10, 0, 30, 0)]])
    cpu.read()
    first = cpu.read()
    assert cpu.read() == first == 25.0


def test_cpu_survives_counter_wrap(tach_sender):
    top = 2**32 - 10
    cpu = make_cpu_ticks(tach_sender, [[(top, 0, top, 0)], [(10, 0, 10, 0)]])
    cpu.read()
    assert cpu.read() == 50.0  # 20 busy, 20 idle across the wrap


# --- find_tach_port -----------------------------------------------------------

def tach_port(device, product="tach device", vid=0x16C0, pid=0x27DD):
    p = FakePort(device, vid=vid, pid=pid)
    p.product = product
    return p


def test_finds_the_v2_device(tach_sender):
    ports = [FakePort("/dev/cu.usbmodem2101", vid=0x239A, pid=0x8036),
             tach_port("/dev/cu.usbmodemscaffold1")]
    with mock.patch("serial.tools.list_ports.comports", return_value=ports):
        assert tach_sender.find_tach_port() == "/dev/cu.usbmodemscaffold1"


def test_shared_test_pid_needs_the_product_name(tach_sender):
    """16c0:27dd is a shared test pair: another CDC gadget may use it too."""
    ports = [tach_port("/dev/cu.usbmodem9", product="someone else's gadget")]
    with mock.patch("serial.tools.list_ports.comports", return_value=ports):
        assert tach_sender.find_tach_port() is None
