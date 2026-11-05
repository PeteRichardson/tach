#!/usr/bin/env python3
"""Tach sender: send live Mac performance data to the Tach display.

CPU and GPU come from `macmon pipe`. Memory comes from host_statistics64().
The script sends one JSON line each second to the PyPortal data port.
See design.md, sections 5.1, 6.1 and 7.

Usage:
    uv run tach-sender.py              # find the PyPortal and send
    uv run tach-sender.py --port /dev/cu.usbmodem14103
    uv run tach-sender.py --stdout     # print the lines, do not use USB
"""
import argparse
import ctypes
import json
import os
import shutil
import subprocess
import sys
import threading
import time

ADAFRUIT_VID = 0x239A
SEND_INTERVAL_S = 1.0
MEM_SAMPLES_PER_SEND = 4  # read memory 4 times each second, send the maximum
MACMON_STALE_S = 5.0
RETRY_S = 2.0
GB = 1024 ** 3


def log(*parts):
    print(time.strftime("%H:%M:%S"), *parts, file=sys.stderr, flush=True)


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

    def __init__(self):
        super().__init__(daemon=True)
        self.lock = threading.Lock()
        self.cpu = None
        self.gpu = None
        self.updated = 0.0
        self.proc = None

    def latest(self):
        with self.lock:
            if time.monotonic() - self.updated > MACMON_STALE_S:
                return None, None
            return self.cpu, self.gpu

    def run(self):
        while True:
            try:
                self.proc = subprocess.Popen(
                    ["macmon", "pipe", "-i", str(int(SEND_INTERVAL_S * 1000))],
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
                log("macmon stopped (exit {}); restarting".format(code))
            except OSError as e:
                log("cannot run macmon:", e)
            time.sleep(RETRY_S)

    def stop(self):
        if self.proc and self.proc.poll() is None:
            self.proc.terminate()


# --- USB serial --------------------------------------------------------------

def find_data_port():
    """Return the device path of the PyPortal data port, or None."""
    from serial.tools import list_ports

    ports = [p for p in list_ports.comports() if p.vid == ADAFRUIT_VID]
    for p in ports:
        names = "{} {}".format(p.interface or "", p.description or "").lower()
        if "data" in names:
            return p.device
    if len(ports) >= 2:
        # macOS often has no interface name. The data interface comes after the
        # console, so its device name has the higher number.
        return sorted(p.device for p in ports)[-1]
    if len(ports) == 1:
        log("found only one Adafruit port ({}): is boot.py on the device?".format(ports[0].device))
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


def main():
    parser = argparse.ArgumentParser(description="Send Mac performance data to the Tach display.")
    parser.add_argument("--port", help="serial port of the PyPortal data channel (default: find it)")
    parser.add_argument("--stdout", action="store_true", help="print the lines; do not use USB")
    args = parser.parse_args()

    if shutil.which("macmon") is None:
        log("macmon not found. Install it: brew install macmon")
        return 1

    memory = Memory()
    macmon = Macmon()
    macmon.start()
    try:
        sink = StdoutSink() if args.stdout else SerialSink(args.port)
    except ImportError:
        log("pyserial not found. Run the script with: uv run tach-sender.py")
        return 1

    step = SEND_INTERVAL_S / MEM_SAMPLES_PER_SEND
    next_sample = time.monotonic()
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
