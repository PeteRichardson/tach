#!/usr/bin/env python3
"""Tach sender: send live Mac performance data to the Tach display.

Two devices, one script:

- The v2 device (RP2350, device/) pulls. It sends {"pull":N} once per frame and
  the script answers with one reply line. CPU comes from Mach ticks, memory from
  host_statistics64(), GPU from `macmon pipe`. See docs/device-v2.md, "Wire".
- The PyPortal (v1) is pushed to. The script sends one JSON line each
  SEND_INTERVAL_S to its data port. CPU and GPU come from `macmon pipe`. See
  design.md, sections 5.1, 6.1 and 7.

Both can be connected at once: a thread answers the v2 device while the main
loop keeps pushing to the PyPortal.

Usage:
    uv run tach-sender.py              # find the devices and serve them
    uv run tach-sender.py --port /dev/cu.usbmodem14103   # PyPortal port
    uv run tach-sender.py --stdout     # print v1 lines, do not use USB
"""
import argparse
import ctypes
import json
import os
import shutil
import signal
import subprocess
import sys
import threading
import time

ADAFRUIT_VID = 0x239A
# The v2 device: the pid.codes test pair for CDC-ACM, so match the product name too.
TACH_VID_PID = (0x16C0, 0x27DD)
TACH_PRODUCT = "tach device"
PROTOCOL_VERSION = 2
# macmon's best rate is about 9 Hz (docs/device-v2.md, Mac-side sources). The
# pull path asks for 10 Hz; the v1 path keeps its 2 Hz.
PULL_MACMON_INTERVAL_MS = 100
# How long the pull loop waits for bytes before it reads memory anyway, so the
# reply after a gap in the pulls still carries the low and high of the gap.
PULL_WAIT_S = 0.05
SEND_INTERVAL_S = 0.5
MEM_SAMPLES_PER_SEND = 4  # read memory 4 times for each message, send the maximum
MACMON_STALE_S = 5.0
RETRY_S = 2.0
GB = 1024 ** 3


def log(*parts):
    print(time.strftime("%H:%M:%S"), *parts, file=sys.stderr, flush=True)


_last_log_once = None


def log_once(*parts):
    """log(), but not again while the message stays the same."""
    global _last_log_once
    if parts != _last_log_once:
        _last_log_once = parts
        log(*parts)


# --- Memory (design.md 5.1) ---------------------------------------------------

class VMStatistics64(ctypes.Structure):
    # struct vm_statistics64 from <mach/vm_statistics.h>
    _fields_ = [
        ("free_count", ctypes.c_uint32),
        ("active_count", ctypes.c_uint32),
        ("inactive_count", ctypes.c_uint32),
        ("wire_count", ctypes.c_uint32),
        ("zero_fill_count", ctypes.c_uint64),
        ("reactivations", ctypes.c_uint64),
        ("pageins", ctypes.c_uint64),
        ("pageouts", ctypes.c_uint64),
        ("faults", ctypes.c_uint64),
        ("cow_faults", ctypes.c_uint64),
        ("lookups", ctypes.c_uint64),
        ("hits", ctypes.c_uint64),
        ("purges", ctypes.c_uint64),
        ("purgeable_count", ctypes.c_uint32),
        ("speculative_count", ctypes.c_uint32),
        ("decompressions", ctypes.c_uint64),
        ("compressions", ctypes.c_uint64),
        ("swapins", ctypes.c_uint64),
        ("swapouts", ctypes.c_uint64),
        ("compressor_page_count", ctypes.c_uint32),
        ("throttled_count", ctypes.c_uint32),
        ("external_page_count", ctypes.c_uint32),
        ("internal_page_count", ctypes.c_uint32),
        ("total_uncompressed_pages_in_compressor", ctypes.c_uint64),
    ]


class Memory:
    """Memory used, as Activity Monitor shows it: app + wired + compressed."""

    HOST_VM_INFO64 = 4

    def __init__(self):
        self.libc = ctypes.CDLL("/usr/lib/libSystem.B.dylib")
        self.libc.mach_host_self.restype = ctypes.c_uint32
        self.libc.host_statistics64.argtypes = [
            ctypes.c_uint32,
            ctypes.c_int,
            ctypes.POINTER(VMStatistics64),
            ctypes.POINTER(ctypes.c_uint32),
        ]
        self.host = self.libc.mach_host_self()  # get once: each call adds a port reference
        self.page_size = os.sysconf("SC_PAGE_SIZE")
        self.total_gb = self._memsize() / GB

    def _memsize(self):
        size = ctypes.c_uint64()
        length = ctypes.c_size_t(ctypes.sizeof(size))
        if self.libc.sysctlbyname(b"hw.memsize", ctypes.byref(size), ctypes.byref(length), None, 0):
            raise OSError("sysctlbyname(hw.memsize) failed")
        return size.value

    def used_gb(self):
        stats = VMStatistics64()
        count = ctypes.c_uint32(ctypes.sizeof(stats) // 4)
        kr = self.libc.host_statistics64(self.host, self.HOST_VM_INFO64, ctypes.byref(stats), ctypes.byref(count))
        if kr != 0:
            raise OSError("host_statistics64 failed: {}".format(kr))
        app = stats.internal_page_count - stats.purgeable_count
        pages = app + stats.wire_count + stats.compressor_page_count
        return pages * self.page_size / GB


class MemWindow:
    """Memory reads since the last reply: the newest, the low, the high and the newest stamp."""

    def __init__(self):
        self.last = None
        self.lo = None
        self.hi = None
        self.stamp_ms = None

    def add(self, gb, stamp_ms):
        self.last = gb
        self.lo = gb if self.lo is None else min(self.lo, gb)
        self.hi = gb if self.hi is None else max(self.hi, gb)
        self.stamp_ms = stamp_ms

    def take(self):
        """Return (mem, lo, hi, stamp_ms) and start a new window at the newest read."""
        out = (self.last, self.lo, self.hi, self.stamp_ms)
        self.lo = self.hi = self.last
        return out


# --- CPU from Mach ticks (ADR-0007) -------------------------------------------

class CpuTicks:
    """CPU busy percent from host_processor_info(PROCESSOR_CPU_LOAD_INFO), over all cores.

    busy / (busy + idle) between two reads, the Activity Monitor definition.
    Per-core counters, not host_statistics(HOST_CPU_LOAD_INFO): on macOS 27 the
    host-wide sum moves only about once a second, while the per-core counters
    move every few milliseconds (measured 2026-11-12).
    """

    PROCESSOR_CPU_LOAD_INFO = 2
    CPU_STATE_MAX = 4  # user, system, idle, nice

    def __init__(self):
        self.libc = ctypes.CDLL("/usr/lib/libSystem.B.dylib")
        self.libc.mach_host_self.restype = ctypes.c_uint32
        self.libc.mach_task_self.restype = ctypes.c_uint32
        self.libc.host_processor_info.argtypes = [
            ctypes.c_uint32,
            ctypes.c_int,
            ctypes.POINTER(ctypes.c_uint32),
            ctypes.POINTER(ctypes.POINTER(ctypes.c_uint32)),
            ctypes.POINTER(ctypes.c_uint32),
        ]
        self.libc.vm_deallocate.argtypes = [ctypes.c_uint32, ctypes.c_size_t, ctypes.c_size_t]
        self.host = self.libc.mach_host_self()  # get once: each call adds a port reference
        self.task = self.libc.mach_task_self()
        self.prev = None
        self.percent = None

    def ticks(self):
        """Return [(busy, idle), ...] tick counts since boot, one pair per core."""
        cores = ctypes.c_uint32()
        info = ctypes.POINTER(ctypes.c_uint32)()
        count = ctypes.c_uint32()
        kr = self.libc.host_processor_info(
            self.host, self.PROCESSOR_CPU_LOAD_INFO, ctypes.byref(cores), ctypes.byref(info), ctypes.byref(count)
        )
        if kr != 0:
            raise OSError("host_processor_info failed: {}".format(kr))
        try:
            out = []
            for core in range(cores.value):
                user, system, idle, nice = info[core * self.CPU_STATE_MAX:(core + 1) * self.CPU_STATE_MAX]
                out.append((user + system + nice, idle))
            return out
        finally:
            # The kernel hands over a fresh buffer on each call.
            addr = ctypes.cast(info, ctypes.c_void_p).value
            self.libc.vm_deallocate(self.task, addr, count.value * ctypes.sizeof(ctypes.c_uint32))

    def read(self):
        """Return busy percent since the previous read. None on the first read.

        If no tick passed since the previous read, return the previous value:
        the window was too short to measure, not idle.
        """
        now = self.ticks()
        if self.prev is not None and len(self.prev) == len(now):
            # Counters are 32-bit and wrap, so take each core's delta on its own.
            d_busy = sum((b - pb) & 0xFFFFFFFF for (b, _), (pb, _) in zip(now, self.prev))
            d_idle = sum((i - pi) & 0xFFFFFFFF for (_, i), (_, pi) in zip(now, self.prev))
            if d_busy + d_idle:
                self.percent = 100.0 * d_busy / (d_busy + d_idle)
        self.prev = now
        return self.percent


# --- CPU and GPU from macmon -------------------------------------------------

def cpu_gpu_from_macmon(sample):
    """Return (cpu %, gpu %) from one `macmon pipe` JSON object. Values are 0-1 ratios."""
    cpu = sample.get("cpu_active_ratio")
    gpu = sample.get("gpu_active_ratio")
    # Older macmon versions: [freq_mhz, ratio] arrays.
    if cpu is None and "ecpu_usage" in sample and "pcpu_usage" in sample:
        cpu = (sample["ecpu_usage"][1] + sample["pcpu_usage"][1]) / 2
    if gpu is None and "gpu_usage" in sample:
        gpu = sample["gpu_usage"][1]
    return (
        None if cpu is None else cpu * 100,
        None if gpu is None else gpu * 100,
    )


class Macmon(threading.Thread):
    """Run `macmon pipe` and keep the last CPU and GPU values. Restart it if it stops."""

    def __init__(self, interval_ms=int(SEND_INTERVAL_S * 1000)):
        super().__init__(daemon=True)
        self.lock = threading.Lock()
        self.cpu = None
        self.gpu = None
        self.updated = 0.0
        self.proc = None
        self.interval_ms = interval_ms
        self.restarting = False
        self.stopping = threading.Event()

    def latest(self):
        with self.lock:
            if time.monotonic() - self.updated > MACMON_STALE_S:
                return None, None
            return self.cpu, self.gpu

    def gpu_sample(self):
        """Return (gpu %, arrival in ms on the monotonic clock), or None when macmon is stale."""
        with self.lock:
            if self.gpu is None or time.monotonic() - self.updated > MACMON_STALE_S:
                return None
            return self.gpu, int(self.updated * 1000)

    def set_interval(self, interval_ms):
        """Change the macmon interval. macmon restarts with the new one."""
        if interval_ms == self.interval_ms:
            return
        self.interval_ms = interval_ms
        self.restarting = True
        if self.proc and self.proc.poll() is None:
            self.proc.terminate()

    def run(self):
        while not self.stopping.is_set():
            try:
                self.proc = subprocess.Popen(
                    ["macmon", "pipe", "-i", str(self.interval_ms)],
                    stdout=subprocess.PIPE,
                    text=True,
                )
                for line in self.proc.stdout:
                    try:
                        cpu, gpu = cpu_gpu_from_macmon(json.loads(line))
                    except (ValueError, TypeError, KeyError, IndexError):
                        continue
                    with self.lock:
                        self.cpu, self.gpu = cpu, gpu
                        self.updated = time.monotonic()
                code = self.proc.wait()
                if self.stopping.is_set():
                    return  # stop() ended macmon: not an error
                if self.restarting:
                    self.restarting = False
                    continue  # set_interval() ended macmon: start again at once
                log("macmon stopped (exit {}); restarting".format(code))
            except OSError as e:
                log("cannot run macmon:", e)
            self.stopping.wait(RETRY_S)

    def stop(self):
        self.stopping.set()
        if self.proc and self.proc.poll() is None:
            self.proc.terminate()


# --- USB serial --------------------------------------------------------------

def find_data_port():
    """Return the device path of the PyPortal data port, or None.

    All CircuitPython boards use the Adafruit vendor ID, so group the ports by
    board. A board with Tach's boot.py has two ports: the console and the data
    port. Group by USB serial number; when macOS omits it, fall back to the
    IORegistry location (p.location), which is shared by both ports of one
    device.
    """
    from serial.tools import list_ports

    boards = {}
    for p in list_ports.comports():
        if p.vid == ADAFRUIT_VID:
            boards.setdefault(p.serial_number or p.location, []).append(p)

    candidates = []
    for ports in boards.values():
        if len(ports) >= 2:
            candidates.append(ports)
    if not candidates:
        if boards:
            log_once("no Adafruit board has a data port: is boot.py on the device?")
        return None

    # Prefer a PyPortal. The description is the USB product name.
    candidates.sort(key=lambda ports: "pyportal" not in (ports[0].description or "").lower())
    if len(candidates) > 1:
        log_once("{} boards have a data port; use --port to select one".format(len(candidates)))
    # macOS pyserial does not populate p.interface, so the numeric heuristic
    # is the only way to distinguish console from data port: the data
    # interface comes after the console, so its device name has the higher number.
    return sorted(p.device for p in candidates[0])[-1]


def find_tach_port():
    """Return the device path of the v2 Tach device, or None."""
    from serial.tools import list_ports

    for p in list_ports.comports():
        if (p.vid, p.pid) == TACH_VID_PID and p.product == TACH_PRODUCT:
            return p.device
    return None


class SerialSink:
    """Send lines to the PyPortal. Connect again after a disconnect."""

    def __init__(self, port=None):
        import serial

        self.serial = serial
        self.fixed_port = port
        self.conn = None
        self.next_try = 0.0

    def send(self, line):
        if self.conn is None and not self._connect():
            return
        try:
            self.conn.write(line.encode("utf-8"))
        except (self.serial.SerialException, OSError) as e:
            log("disconnected:", e)
            self._close()

    def _connect(self):
        now = time.monotonic()
        if now < self.next_try:
            return False
        self.next_try = now + RETRY_S
        port = self.fixed_port or find_data_port()
        if port is None:
            return False
        try:
            self.conn = self.serial.Serial(port, 115200, timeout=0, write_timeout=1)
        except (self.serial.SerialException, OSError) as e:
            log("cannot open {}: {}".format(port, e))
            return False
        log("connected to", port)
        return True

    def _close(self):
        try:
            self.conn.close()
        except Exception:
            pass
        self.conn = None

    def close(self):
        if self.conn is not None:
            self._close()


class StdoutSink:
    def send(self, line):
        sys.stdout.write(line)
        sys.stdout.flush()

    def close(self):
        pass


# --- Main loop ---------------------------------------------------------------

def build_message(cpu, gpu, mem, mem_total):
    msg = {}
    if cpu is not None:
        msg["cpu"] = round(cpu, 1)
    if gpu is not None:
        msg["gpu"] = round(gpu, 1)
    msg["mem"] = round(mem, 2)
    msg["mem_total"] = round(mem_total, 1)
    return json.dumps(msg, separators=(",", ":")) + "\n"


def now_ms():
    """Mac monotonic clock in ms: the stamp for every value on the wire (ADR-0005)."""
    return int(time.monotonic() * 1000)


def parse_pull(line):
    """Return N from a {"pull":N} line, or None if the line is not a pull."""
    try:
        seq = json.loads(line)["pull"]
    except (ValueError, TypeError, KeyError):
        return None
    if isinstance(seq, bool) or not isinstance(seq, int):
        return None
    return seq


def build_reply(seq, t, cpu, tc, gpu_sample, mem_window, mem_total):
    """One reply line for pull `seq` (docs/device-v2.md, "Wire").

    `pull` echoes the request. `t`, `tc`, `tg` and `tm` are Mac monotonic ms:
    when the reply was built, and when each value was read. A value that is
    unknown is left out, not sent as zero. `ok` is false when macmon is stale.
    """
    msg = {"pull": seq, "v": PROTOCOL_VERSION, "t": t, "hz": round(1000 / PULL_MACMON_INTERVAL_MS, 1)}
    if cpu is not None:
        msg["cpu"] = round(cpu, 1)
        msg["tc"] = tc
    if gpu_sample is not None:
        msg["gpu"] = round(gpu_sample[0], 1)
        msg["tg"] = gpu_sample[1]
    mem, lo, hi, tm = mem_window
    if mem is not None:
        msg["mem"] = round(mem, 2)
        msg["mem_lo"] = round(lo, 2)
        msg["mem_hi"] = round(hi, 2)
        msg["tm"] = tm
    msg["mem_total"] = round(mem_total, 1)
    msg["ok"] = gpu_sample is not None
    return json.dumps(msg, separators=(",", ":")) + "\n"


class PullServer(threading.Thread):
    """Find the v2 device and answer its pulls. Look again after a disconnect."""

    def __init__(self, memory, macmon):
        super().__init__(daemon=True)
        self.memory = memory
        self.macmon = macmon
        self.cpu_ticks = CpuTicks()

    def run(self):
        while True:
            port = find_tach_port()
            if port is not None:
                self.macmon.set_interval(PULL_MACMON_INTERVAL_MS)
                serve_pulls(port, self.memory, self.macmon, self.cpu_ticks)
                self.macmon.set_interval(int(SEND_INTERVAL_S * 1000))
            time.sleep(RETRY_S)


def serve_pulls(port, memory, macmon, cpu_ticks):
    """Answer pulls from the v2 device until it disconnects."""
    import serial

    try:
        conn = serial.Serial(port, timeout=PULL_WAIT_S, write_timeout=1)
    except (serial.SerialException, OSError) as e:
        log("cannot open {}: {}".format(port, e))
        return
    log("connected to", port, "(pull)")
    mem = MemWindow()
    pending = b""
    try:
        while True:
            pending += conn.read(conn.in_waiting or 1)
            mem.add(memory.used_gb(), now_ms())
            *lines, pending = pending.split(b"\n")
            # Answer only the newest pull. The device drops a reply to an older one.
            seqs = [s for s in map(parse_pull, lines) if s is not None]
            if not seqs:
                continue
            cpu = cpu_ticks.read()
            tc = now_ms()
            reply = build_reply(seqs[-1], now_ms(), cpu, tc, macmon.gpu_sample(), mem.take(), memory.total_gb)
            conn.write(reply.encode("utf-8"))
    except (serial.SerialException, OSError) as e:
        log("disconnected:", e)
    finally:
        conn.close()


def main():
    parser = argparse.ArgumentParser(description="Send Mac performance data to the Tach display.")
    parser.add_argument("--port", help="serial port of the PyPortal data channel (default: find it)")
    parser.add_argument("--stdout", action="store_true", help="print the lines; do not use USB")
    args = parser.parse_args()
    # launchd stops the agent with SIGTERM. Exit through the finally block to stop macmon.
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))

    if shutil.which("macmon") is None:
        log_once("macmon not found. Install it: brew install macmon")
        return 0  # clean exit: launchd should not restart (KeepAlive: SuccessfulExit=false)

    memory = Memory()
    macmon = Macmon()
    macmon.start()
    try:
        sink = StdoutSink() if args.stdout else SerialSink(args.port)
    except ImportError:
        log_once("pyserial not found. Run the script with: uv run tach-sender.py")
        return 0  # clean exit: launchd should not restart (KeepAlive: SuccessfulExit=false)

    step = SEND_INTERVAL_S / MEM_SAMPLES_PER_SEND
    next_sample = time.monotonic()
    if not args.stdout:
        PullServer(memory, macmon).start()
    try:
        while True:
            peak = 0.0
            for _ in range(MEM_SAMPLES_PER_SEND):
                peak = max(peak, memory.used_gb())
                next_sample += step
                delay = next_sample - time.monotonic()
                if delay > 0:
                    time.sleep(delay)
                else:
                    next_sample = time.monotonic()  # far behind: start again from now
            cpu, gpu = macmon.latest()
            sink.send(build_message(cpu, gpu, peak, memory.total_gb))
    except KeyboardInterrupt:
        return 0
    finally:
        macmon.stop()
        sink.close()


if __name__ == "__main__":
    sys.exit(main())
