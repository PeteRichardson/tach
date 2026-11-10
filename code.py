# Tach code.py: show live CPU, GPU and memory use of a Mac.
# The Mac script (tach-sender.py) sends one JSON line each tick on usb_cdc.data.
# See design.md, sections 5, 6.2 and 7.
#
# Libraries (copy to CIRCUITPY/lib): adafruit_display_text, adafruit_bitmap_font.
# Big-number font: put a BDF or PCF font, approximately 36 px high, at BIG_FONT_PATH.
# If the font is missing or damaged, the device uses the scaled built-in font
# and shows NO FONT in the top-left corner.
import array
import json
import math
import time
from collections import namedtuple

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
TIMING = False  # print the work time of each tick on the console
TIMING_TICKS = 20  # ticks for each timing print

# MEM chart: compressed time axis and zoomed y axis (design.md 5.2)
MEM_HISTORY_S = 5 * 60  # age of the left edge of the MEM chart
MEM_GRID_S = (60, 2 * 60, 3 * 60, 4 * 60)  # dotted time lines on the MEM chart
MEM_MIN_SPAN_GB = 1.0  # smallest y span, so that noise does not look large
MEM_STEPS_GB = (0.1, 0.2, 0.5, 1, 2, 5, 10, 20, 50, 100)
MEM_SCALE_COLOR = 0x808080
GRID = 0x404040

SCREEN_W = 320  # PyPortal landscape (design.md 4); the layout is fixed pixels
ROW_H = 80
CHART_H = 70
TEXT_X = 4  # x of the row label and the big number
LABEL_Y = 2  # y of the label top; corner of the row
LABEL_SCALE = 2  # built-in font twice as big for the row label
# Built-in font metrics (terminalio is fixed-size: ~8 px advance, ~11 px
# line). Constants on purpose: the Label text-measurement property changes
# name between adafruit_display_text versions, and the built-in font never
# varies. See tests/test_display_rows.py for the guard.
LABEL_ADV = 8
LABEL_LINE_H = 11
NUMBER_GAP = 5  # vertical gap: label bottom -> number top
NUMBER_Y = LABEL_Y + LABEL_LINE_H * LABEL_SCALE + NUMBER_GAP  # 29
NUMBER_W = 100  # reserved width of the big number (5 chars at ~36 px)
# Static right edge for all three numbers: the "%" and "G" line up in one
# column. Fixed value so the string width changes no layout. 10 px in from
# the slot edge; the worst-case strings ("100%", "97.4G", ~94 px wide) then
# end at x = 0, so a wider font clips at the screen edge.
NUMBER_RIGHT_X = TEXT_X + NUMBER_W - 10  # 94
BOLD_SHIFT = 1  # px: the number is drawn twice, offset right, to fake bold
CHART_GAP = 10  # horizontal gap: number slot -> chart
AXIS_GAP = 6  # horizontal gap: MEM y-axis labels -> chart
CHART_X = TEXT_X + NUMBER_W + CHART_GAP  # 114
CHART_W = SCREEN_W - CHART_X  # 206 columns: ~103 s at 2 Hz (design.md 5)
CHART_Y = (ROW_H - CHART_H) // 2

BG = 0x000000
RULE = 0x303030
STALE_COLOR = 0x505050


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
    """One metric: label in the upper-left corner, big number under it,
    scrolling line chart on the rest of the row."""

    def __init__(self, y, name, color, full, font, scale, fmt):
        if full is None:
            # _y divides by self.full: a None here would hard-fault the device
            # at the first drawn sample.
            raise ValueError("Row needs a numeric full scale")
        self.color = color
        self.full = full
        self.fmt = fmt
        self.group = displayio.Group(y=y)

        name_label = label.Label(terminalio.FONT, text=name, color=color, scale=LABEL_SCALE)
        name_label.anchor_point = (0.0, 0.0)
        name_label.anchored_position = (TEXT_X, LABEL_Y)
        self.group.append(name_label)

        # Right-aligned at the static NUMBER_RIGHT_X so the units (% and G)
        # line up. A wider font than NUMBER_W assumes extends left, under the
        # label, before it could reach the chart.
        self.number = label.Label(font, text="--", color=STALE_COLOR, scale=scale)
        self.number.anchor_point = (1.0, 0.0)
        self.number.anchored_position = (NUMBER_RIGHT_X, NUMBER_Y)
        self.group.append(self.number)
        self._make_chart(color, CHART_X, CHART_Y)

        # Pseudo-bold: the font comes from a user file, so the weight is what
        # the file says. Drawing the same text again BOLD_SHIFT px right
        # thickens every stroke. Both labels share the loaded font, so the
        # extra cost is one small text bitmap per row.
        self.number_bold = label.Label(font, text="--", color=STALE_COLOR, scale=scale)
        self.number_bold.anchor_point = (1.0, 0.0)
        self.number_bold.anchored_position = (NUMBER_RIGHT_X + BOLD_SHIFT, NUMBER_Y)
        self.group.append(self.number_bold)

    def _make_chart(self, color, x, y):
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
            x=x,
            y=y,
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

    def show(self, value, stale):
        if value is not None:
            text = self.fmt(value)
            if text != self.number.text:
                self.number.text = text
                self.number_bold.text = text
        color = STALE_COLOR if stale else self.color
        self.number.color = color
        self.number_bold.color = color


def nice_range(lo, hi):
    """Round lo..hi out to steps, so that the y axis does not move at each tick."""
    if hi - lo < MEM_MIN_SPAN_GB:
        mid = (lo + hi) / 2
        lo, hi = mid - MEM_MIN_SPAN_GB / 2, mid + MEM_MIN_SPAN_GB / 2
    for step in MEM_STEPS_GB:
        if (hi - lo) / step <= 5:
            break
    lo = max(math.floor(lo / step) * step, 0)
    hi = math.ceil(hi / step) * step
    return lo, hi


def log_edges(columns, samples):
    """Return columns + 1 sample ages. Column x (0 = right edge) shows the samples
    with an age from edges[x] to edges[x + 1] - 1. The first columns have one
    sample each; the age grows exponentially to the left edge."""
    samples = max(samples, columns)
    # age(x) = b * (exp(x / b) - 1) has 1 sample per column at x = 0.
    # Find b so that age(columns) = samples. age() decreases when b increases.
    lo, hi = columns / 80, 1e6
    for _ in range(60):
        b = (lo + hi) / 2
        if b * (math.exp(columns / b) - 1) > samples:
            lo = b
        else:
            hi = b
    edges = [0]
    for x in range(1, columns):
        edges.append(max(int(b * (math.exp(x / b) - 1) + 0.5), edges[-1] + 1))
    edges.append(max(samples, edges[-1] + 1))
    return edges


class MemRow(Row):
    """The MEM row: a compressed time axis and a y axis that zooms to the data.

    Samples are in units of 0.01 GB. Two arrays keep each sample: in `his` a gap
    is 0, and in `los` a gap is 0xFFFF. Thus max() and min() skip the gaps."""

    GAP_HI = 0
    GAP_LO = 0xFFFF

    def _make_chart(self, color, x, y):
        self.size = int(MEM_HISTORY_S * 1e9 / TICK_NS)
        self.edges = log_edges(CHART_W, self.size)
        self.size = self.edges[-1]
        self.his = array.array("H", (self.GAP_HI for _ in range(self.size)))
        self.los = array.array("H", (self.GAP_LO for _ in range(self.size)))
        self.head = 0  # next index to write
        self.grid_cols = set()
        for seconds in MEM_GRID_S:
            age = seconds * 1e9 / TICK_NS
            for col, edge in enumerate(self.edges):
                if edge >= age:
                    self.grid_cols.add(CHART_W - 1 - col)
                    break

        self.bitmap = displayio.Bitmap(CHART_W, CHART_H, 4)
        palette = displayio.Palette(4)
        palette[0] = BG
        palette[1] = dim(color)
        palette[2] = color
        palette[3] = GRID
        self.group.append(displayio.TileGrid(self.bitmap, pixel_shader=palette, x=x, y=y))

        # The y-axis limits go to the left of the chart, right-aligned to it,
        # so the line can never cover them. They share the column with the
        # big number without touching it: the number sits between them, and
        # the bottom label hangs 2 px below the chart to clear its base.
        self.top = label.Label(terminalio.FONT, text="", color=MEM_SCALE_COLOR)
        self.top.anchor_point = (1.0, 0.0)
        self.top.anchored_position = (x - AXIS_GAP, y)
        self.group.append(self.top)
        self.bottom = label.Label(terminalio.FONT, text="", color=MEM_SCALE_COLOR)
        self.bottom.anchor_point = (1.0, 1.0)
        self.bottom.anchored_position = (x - AXIS_GAP, ROW_H - 2)
        self.group.append(self.bottom)

    def _agg(self, values, a, b, fn):
        """fn() of the samples with an age from a to b - 1."""
        start = (self.head - b) % self.size
        end = start + (b - a)
        view = memoryview(values)
        if end <= self.size:
            return fn(view[start:end])
        return fn(fn(view[start:]), fn(view[: end - self.size]))

    def push(self, value):
        if value is None:
            self.his[self.head] = self.GAP_HI
            self.los[self.head] = self.GAP_LO
        else:
            v = min(max(int(value * 100 + 0.5), 1), 0xFFFE)
            self.his[self.head] = v
            self.los[self.head] = v
        self.head = (self.head + 1) % self.size
        self._redraw()

    def _redraw(self):
        cols = []
        lo_all, hi_all = self.GAP_LO, self.GAP_HI
        for x in range(CHART_W):
            a, b = self.edges[x], self.edges[x + 1]
            hi = self._agg(self.his, a, b, max)
            if hi == self.GAP_HI:
                cols.append(None)
                continue
            lo = self._agg(self.los, a, b, min)
            cols.append((lo, hi))
            lo_all = min(lo_all, lo)
            hi_all = max(hi_all, hi)

        if hi_all == self.GAP_HI:
            bitmaptools.fill_region(self.bitmap, 0, 0, CHART_W, CHART_H, 0)
            self._set_scale("", "")
            return
        lo_gb, hi_gb = nice_range(lo_all / 100, hi_all / 100)
        self._set_scale(fmt_gb(hi_gb), fmt_gb(lo_gb))

        # y of a value in 0.01 GB
        k = (CHART_H - 1) / ((hi_gb - lo_gb) * 100)
        base = lo_gb * 100

        def y_of(v):
            return min(max(CHART_H - 1 - round((v - base) * k), 0), CHART_H - 1)

        ys = [None if c is None else (y_of(c[1]), y_of(c[0])) for c in cols]
        for x in range(CHART_W):
            col = CHART_W - 1 - x
            bitmaptools.fill_region(self.bitmap, col, 0, col + 1, CHART_H, 0)
            if col in self.grid_cols:
                for gy in range(0, CHART_H, 3):
                    self.bitmap[col, gy] = 3
            if ys[x] is None:
                continue
            top, bottom = ys[x]
            bitmaptools.fill_region(self.bitmap, col, top, col + 1, CHART_H, 1)
            older = ys[x + 1] if x + 1 < CHART_W else None
            if older is not None:
                # join the line to the next older column
                top, bottom = min(top, older[1]), max(bottom, older[0])
            bitmaptools.fill_region(self.bitmap, col, top, col + 1, bottom + 1, 2)

    def _set_scale(self, top, bottom):
        if self.top.text != top:
            self.top.text = top
        if self.bottom.text != bottom:
            self.bottom.text = bottom


# The single definition of the displayed metrics: key, label, colour, chart
# full scale, row class, number formatter. MemRow autoscales and ignores its
# full_scale. build_display() and the parse loop read this table.
Metric = namedtuple("Metric", "key label color full_scale row_class fmt")
METRICS = (
    Metric("cpu", "CPU", 0x40C0FF, 100.0, Row, fmt_pct),
    Metric("gpu", "GPU", 0x60FF60, 100.0, Row, fmt_pct),
    Metric("mem", "MEM", 0xFFB040, 1.0, MemRow, fmt_gb),
)
METRIC_KEYS = tuple(m.key for m in METRICS)


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
        return font, 1, True
    except Exception as e:
        # Any font fault falls back; the fault list is in tests/test_font_fallback.py.
        print("Tach: cannot load", BIG_FONT_PATH, "-", e)
        print("Tach: using the scaled built-in font")
        return terminalio.FONT, 3, False


def build_display(font, scale, big_font_ok):
    root = displayio.Group()
    rows = {}
    for i, m in enumerate(METRICS):
        row = m.row_class(i * ROW_H, m.label, m.color, m.full_scale, font, scale, m.fmt)
        rows[m.key] = row
        root.append(row.group)

    if not big_font_ok:
        # Global chrome on root, like the rules: the console print in
        # load_big_font() is invisible unless the console port is open, so
        # mark the fallback at the top of the screen. The corner belongs to
        # the CPU label, so the notice starts to the right of it.
        warn_x = TEXT_X + LABEL_ADV * LABEL_SCALE * len(METRICS[0].label) + 4
        warn = label.Label(terminalio.FONT, text="NO FONT", color=MEM_SCALE_COLOR)
        warn.anchor_point = (0.0, 0.0)
        warn.anchored_position = (warn_x, LABEL_Y)
        root.append(warn)

    rule = displayio.Bitmap(SCREEN_W, 1, 1)
    rule_palette = displayio.Palette(1)
    rule_palette[0] = RULE
    for i in range(1, len(METRICS)):
        root.append(displayio.TileGrid(rule, pixel_shader=rule_palette, y=i * ROW_H))
    return root, rows


def main():
    display = board.DISPLAY
    font, scale, big_font_ok = load_big_font()
    root, rows = build_display(font, scale, big_font_ok)
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
    keys = METRIC_KEYS  # local bind: the parse loop reads it per received line
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
                for key in keys:
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
