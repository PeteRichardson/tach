# Tach code.py: show live CPU, GPU and memory use of a Mac.
# The Mac script (tach-sender.py) sends one JSON line each tick on usb_cdc.data.
# See design.md, sections 5, 6.2 and 7.
#
# Libraries (copy to CIRCUITPY/lib): adafruit_display_text, adafruit_bitmap_font.
# Big-number font: put a BDF or PCF font, approximately 36 px high, at BIG_FONT_PATH.
# If the font is missing, the device uses the scaled built-in font.
import json
import math
import time

import bitmaptools
import board
import displayio
import terminalio
import usb_cdc
from adafruit_display_text import label

DEMO = False  # True: show generated test data. Do not read USB.
BIG_FONT_PATH = "/fonts/tach-36.bdf"

TICK_NS = 500_000_000  # must agree with SEND_INTERVAL_S in tach-sender.py
STALE_NS = 5_000_000_000
MAX_LINE = 256
TIMING = True  # print the work time of each tick on the console
TIMING_TICKS = 20  # ticks for each timing print

ROW_H = 80
LABEL_W = 40
NUMBER_W = 110
CHART_W = 170
CHART_H = 70

BG = 0x000000
RULE = 0x303030
STALE_COLOR = 0x505050

# key, label, colour, chart full scale (memory scale comes from mem_total)
METRICS = (
    ("cpu", "CPU", 0x40C0FF, 100.0),
    ("gpu", "GPU", 0x60FF60, 100.0),
    ("mem", "MEM", 0xFFB040, 128.0),
)


def dim(color):
    r, g, b = (color >> 16) & 0xFF, (color >> 8) & 0xFF, color & 0xFF
    return ((r // 4) << 16) | ((g // 4) << 8) | (b // 4)


def fmt_pct(value):
    return "{:.0f}%".format(value)


def fmt_gb(value):
    # 5 characters maximum: "97.4G", "128G"
    if value < 99.95:
        return "{:.1f}G".format(value)
    return "{:.0f}G".format(value)


class Row:
    """One metric: small label, big number, scrolling line chart."""

    def __init__(self, y, name, color, full, font, scale):
        self.color = color
        self.full = full
        self.fmt = fmt_gb if name == "MEM" else fmt_pct
        self.group = displayio.Group(y=y)

        name_label = label.Label(terminalio.FONT, text=name, color=color)
        name_label.anchor_point = (0.0, 0.5)
        name_label.anchored_position = (4, ROW_H // 2)
        self.group.append(name_label)

        self.number = label.Label(font, text="--", color=STALE_COLOR, scale=scale)
        self.number.anchor_point = (1.0, 0.5)
        self.number.anchored_position = (LABEL_W + NUMBER_W - 6, ROW_H // 2)
        self.group.append(self.number)

        # The chart bitmap is a ring of columns. Each tile of the TileGrid is one
        # bitmap column, so a scroll only changes tile indices, not pixels.
        self.bitmap = displayio.Bitmap(CHART_W, CHART_H, 3)
        palette = displayio.Palette(3)
        palette[0] = BG
        palette[1] = dim(color)
        palette[2] = color
        self.grid = displayio.TileGrid(
            self.bitmap,
            pixel_shader=palette,
            width=CHART_W,
            height=1,
            tile_width=1,
            tile_height=CHART_H,
            x=LABEL_W + NUMBER_W,
            y=(ROW_H - CHART_H) // 2,
        )
        self.group.append(self.grid)

        self.samples = [None] * CHART_W  # ring buffer of raw values
        self.head = 0  # next column to write; also the oldest column
        self._place_tiles()

    def _place_tiles(self):
        for x in range(CHART_W):
            self.grid[x] = (self.head + x) % CHART_W

    def _y(self, value):
        frac = min(max(value / self.full, 0.0), 1.0)
        return CHART_H - 1 - round(frac * (CHART_H - 1))

    def _draw_column(self, col, value, prev):
        bitmaptools.fill_region(self.bitmap, col, 0, col + 1, CHART_H, 0)
        if value is None:
            return
        y = self._y(value)
        bitmaptools.fill_region(self.bitmap, col, y, col + 1, CHART_H, 1)
        top = bottom = y
        if prev is not None:
            prev_y = self._y(prev)
            top, bottom = min(y, prev_y), max(y, prev_y)
        bitmaptools.fill_region(self.bitmap, col, top, col + 1, bottom + 1, 2)

    def push(self, value):
        """Add one sample at the right edge. None makes a gap in the chart."""
        col = self.head
        self.samples[col] = value
        self._draw_column(col, value, self.samples[col - 1])
        self.head = (col + 1) % CHART_W
        self._place_tiles()

    def rescale(self, full):
        if full <= 0 or full == self.full:
            return
        self.full = full
        for i in range(CHART_W):
            col = (self.head + i) % CHART_W
            prev = self.samples[col - 1] if i else None
            self._draw_column(col, self.samples[col], prev)

    def show(self, value, stale):
        if value is not None:
            text = self.fmt(value)
            if text != self.number.text:
                self.number.text = text
        self.number.color = STALE_COLOR if stale else self.color


class LineReader:
    """Read lines from a serial port without blocking."""

    def __init__(self, port):
        self.port = port
        self.port.timeout = 0
        self.buf = b""

    def lines(self):
        n = self.port.in_waiting
        if not n:
            return []
        self.buf += self.port.read(n)
        parts = self.buf.split(b"\n")
        self.buf = parts.pop()
        if len(self.buf) > MAX_LINE:
            self.buf = b""  # no newline in a long run of bytes: drop it
        return [p for p in parts if 0 < len(p) <= MAX_LINE]


def parse(line):
    """Return a dict, or None if the line is not a JSON object."""
    try:
        msg = json.loads(line.decode("utf-8"))
    except ValueError:
        return None
    return msg if isinstance(msg, dict) else None


def demo_message(t):
    return {
        "cpu": 50 + 40 * math.sin(t / 7),
        "gpu": 50 + 45 * math.sin(t / 11 + 1),
        "mem": 70 + 40 * math.sin(t / 40),
        "mem_total": 128.0,
    }


def load_big_font():
    try:
        from adafruit_bitmap_font import bitmap_font

        font = bitmap_font.load_font(BIG_FONT_PATH)
        font.load_glyphs("0123456789.%G-")
        return font, 1
    except (OSError, ImportError) as e:
        print("Tach: cannot load", BIG_FONT_PATH, "-", e)
        print("Tach: using the scaled built-in font")
        return terminalio.FONT, 3


def build_display(font, scale):
    root = displayio.Group()
    rows = {}
    for i, (key, name, color, full) in enumerate(METRICS):
        row = Row(i * ROW_H, name, color, full, font, scale)
        rows[key] = row
        root.append(row.group)

    rule = displayio.Bitmap(LABEL_W + NUMBER_W + CHART_W, 1, 1)
    rule_palette = displayio.Palette(1)
    rule_palette[0] = RULE
    for i in range(1, len(METRICS)):
        root.append(displayio.TileGrid(rule, pixel_shader=rule_palette, y=i * ROW_H))
    return root, rows


def main():
    display = board.DISPLAY
    font, scale = load_big_font()
    root, rows = build_display(font, scale)
    display.auto_refresh = False
    display.root_group = root
    display.refresh()

    reader = None
    if not DEMO:
        if usb_cdc.data is None:
            print("Tach: usb_cdc.data is off. Copy boot.py, then press RESET.")
        else:
            reader = LineReader(usb_cdc.data)

    latest = {}
    last_rx = None
    tick = 0
    work_total = 0
    work_max = 0
    next_tick = time.monotonic_ns() + TICK_NS

    while True:
        if reader is not None:
            for line in reader.lines():
                msg = parse(line)
                if msg is None:
                    continue
                for key in ("cpu", "gpu", "mem", "mem_total"):
                    value = msg.get(key)
                    if isinstance(value, (int, float)):
                        latest[key] = float(value)
                last_rx = time.monotonic_ns()

        now = time.monotonic_ns()
        if now >= next_tick:
            next_tick += TICK_NS
            if next_tick <= now:  # far behind: start again from now
                next_tick = now + TICK_NS
            tick += 1

            if DEMO:
                latest = demo_message(tick)
                last_rx = now

            stale = last_rx is None or now - last_rx > STALE_NS
            if "mem_total" in latest:
                rows["mem"].rescale(latest["mem_total"])
            for key, row in rows.items():
                value = None if stale else latest.get(key)
                row.push(value)
                row.show(value, stale)
            display.refresh()

            if TIMING:
                work = time.monotonic_ns() - now
                work_total += work
                work_max = max(work_max, work)
                if tick % TIMING_TICKS == 0:
                    print(
                        "Tach: tick work ms: avg {:.0f} max {:.0f} (tick {:.0f})".format(
                            work_total / TIMING_TICKS / 1e6, work_max / 1e6, TICK_NS / 1e6
                        )
                    )
                    work_total = 0
                    work_max = 0

        time.sleep(0.01)


main()
