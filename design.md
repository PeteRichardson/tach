# Tach — design

Status: v0.1 done, 2026-11-05. The display updates two times each second.

> **v2 is in design.** This document describes v1, the Adafruit PyPortal build, and stays as the
> record of what shipped. The next design — RP2350, RA8875 colour panel, smooth chart motion — is
> in [docs/device-v2.md](docs/device-v2.md), with decisions in
> [docs/adr/](docs/adr/). Do not edit v1 behaviour here to match v2.

## 1. Purpose

Tach is a small external display that shows live performance data for a Mac.
It connects to the Mac with one USB cable. The cable supplies power and data.

The main use is to monitor the Mac while it runs local LLMs. Memory use is the
most important value. The display replaces a TMOG window on the Mac screen.

## 2. Goals

- Show CPU, GPU and memory use. Each value has a big number and a line chart.
- Update two times each second (each 0.5 s).
- Use only one USB cable. Do not use Wi-Fi.
- Do not require `sudo` on the Mac.
- Keep the device code simple, so that it is easy to change.

## 3. Out of scope (for now)

- Wi-Fi or Bluetooth connection.
- Windows or Linux hosts.
- Intel Macs. The design uses `macmon`, which is for Apple Silicon only.
- Settings on the device. The Mac script sends all the data that the device shows.

## 4. Hardware

**Adafruit PyPortal**

| Item | Value |
|---|---|
| Microcontroller | ATSAMD51J20 (120 MHz Cortex-M4) |
| RAM | 192 KB |
| Display | 3.2" TFT, 320 × 240 pixels, resistive touch |
| Connection | USB (micro-B on the original model, USB-C on newer models) |
| Software | CircuitPython |

The PyPortal Titano (3.5", 480 × 320) can also run Tach. The layout must scale
to that size.

### 4.1 Alternatives that we did not select

| Board | Why not |
|---|---|
| Seeed Wio Terminal | Good second choice. It has a case and buttons, but its screen is smaller (2.4"). Its best software support is for Arduino, not CircuitPython. |
| Raspberry Pi Pico 2 + LCD | It has more RAM and speed, but it needs wiring and an enclosure. Use it only for a larger custom display. |
| Turing Smart Screen | It does not need assembly, but its vendor software is for Windows only. The open-source driver has reports of problems on macOS. |
| USB-C monitor or iPad with Sidecar | These are reliable, but they are not a dedicated, small device. |

## 5. Display layout

The display is in landscape mode: 320 × 240 pixels.

There are 3 rows. Each row is 80 pixels high. The small label sits in the
upper-left corner of the row, the big number under it, and the chart fills the
rest of the row:

```
|<- 4 ->|<----- 100 ----->|-10-|<----------- 206 ----------->|
 CPU                         ╱╲__╱‾‾╲___╱╲
 78%             whitespace line chart, ~70 px high
 label + number             206 px wide = ~103 s at 2 Hz
```

```
┌────────────────────────────────────────────┐
│ CPU                       ╱╲__╱‾‾╲___╱╲__ │
│ 78%                                        │
├────────────────────────────────────────────┤
│ GPU                       ___╱‾‾‾‾‾‾‾‾‾╲_ │
│ 92%                                        │
├────────────────────────────────────────────┤
│   130G                ‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾ │
│ MEM  97.4G    ╱‾‾‾‾‾‾‾‾‾‾╲___╱‾‾‾‾‾‾   │
│    90.0G   ╱╱                              │
└────────────────────────────────────────────┘
```

Rules:

- **Label:** 3 characters, the built-in font at twice its size, in the
  upper-left corner of the row.
- **Big number:** 5 characters maximum, for example `100%` or `97.4G`,
  under the label with a 5 px gap, right-aligned at a static x
  (`NUMBER_RIGHT_X` = 94). The `%` and
  `G` units line up in one column across the rows, and because x is static,
  neither the chart nor the units column moves when the value changes. Use a
  smooth bitmap font, approximately 36 px high, with `adafruit_bitmap_font`. Do
  not use the scaled built-in `terminalio` font, because it is blocky. The only
  exception is the fallback when the big font fails to load (see 6.2).
  The number is drawn twice, 1 px right (`BOLD_SHIFT`), for a bold effect; the
  weight of a BDF file cannot be changed in code. For crisper bold, supply a
  bold BDF at the same path and set `BOLD_SHIFT = 0`.
- **Chart:** One pixel column for each sample. The chart runs from after the
  number slot to the right screen edge. At 2 Hz, 206 columns show
  approximately 103 seconds of history. (The MEM chart is different. See 5.2.) The chart scrolls from right to left.
  A faster rate gives less history: at 4 Hz, the chart shows only ~51 seconds.
- **Memory unit:** Show GB, not MB. A Mac for LLMs can have 128 GB or more.
- **Chart scale:** CPU and GPU use a fixed scale of 0–100%. The MEM chart
  zooms to its data. See 5.2.
- **Retention nudge:** The display is lit 24/7. Every 5 minutes
  (`PIXEL_SHIFT_NS`) the whole screen shifts by 1 px through four diagonal
  positions (`pixel_shift()`), so the static pixels — labels, the units
  column, the rules — do not stay lit in one spot. At offset 1 the rightmost
  chart column and the bottom row clip by 1 px. See 6.2.

Physical size on the PyPortal: each row is approximately 16 mm high. The big
numbers are approximately 7 mm high. You can read them at arm's length.

A 4th row (60 px each) is possible if the numbers become smaller.

### 5.1 Memory value (decided)

Decision: Tach shows **memory used**, not memory pressure. The goal is to see
how an LLM uses memory while queries run: memory increases when the model
loads, and it changes as the context (KV cache) grows.

Definition: use the same value as "Memory Used" in Activity Monitor:

```
memory used = app memory + wired memory + compressed memory
```

Get the page counts from `host_statistics64()` (or `vm_stat`), and multiply by
the page size (16 KB on Apple Silicon). Do not use `psutil.virtual_memory().used`,
because on macOS it does not give the same value as Activity Monitor.

**Memory-mapped model files (resolved):** Some LLM runtimes (for example
llama.cpp and Ollama with their default `mmap` setting) map the model file into
memory. These pages show as "File-backed pages" in `vm_stat`, tracked by
`external_page_count` in `host_statistics64()`. They are NOT included in the
formula above, which matches Activity Monitor's "Memory Used" exactly —
Activity Monitor also excludes file-backed pages from its total.

Verified on this machine: the current formula gives 133.6 GB, matching
Activity Monitor's "Memory Used" and `total − free − inactive − speculative −
purgeable`. Adding `external_page_count` would overcount by ~80 GB (the
"File-backed pages" from `vm_stat`), diverging from what the user sees in
Activity Monitor.

MLX and runtimes with `mmap` off load weights into app or wired memory, which
the formula counts. Only `mmap`-on runtimes (llama.cpp, Ollama) have the gap,
and it is intentional: the chart shows what Activity Monitor shows. No second
MEM line is added.

**Short spikes:** At 2 samples each second, the chart can miss a short memory
peak. Thus, the Mac script reads memory 4 times for each message (8 times each
second) and sends the maximum value.

### 5.2 MEM chart (decided)

On a Mac with much memory (for example 256 GB), a 0-to-total scale shows a
flat line. And at 2 Hz, 206 columns show only 103 seconds. Thus, the MEM chart
works as in TMOG:

- **Compressed time axis:** The chart shows the last 5 minutes
  (`MEM_HISTORY_S`). The right columns show one sample each. To the left, the
  time of each column increases exponentially:
  `age(x) = b · (eˣᐟᵇ − 1)` samples, where x is the column from the right edge,
  and b gives `age(206)` = 5 minutes. At 2 Hz, the right 82 columns show the
  last minute, and the leftmost column covers approximately 3 seconds.
- **Minimum and maximum:** Each column shows a line from the lowest to the
  highest sample in its period. Thus, short peaks stay visible.
- **Time marks:** Faint dotted lines show 1, 2, 3 and 4 minutes ago
  (`MEM_GRID_S`), because the time scale is not even.
- **Zoomed y axis:** The scale goes from the lowest to the highest value in the
  5 minutes. It is rounded out to a step (0.1, 0.2, 0.5, 1, 2, 5 … GB) that
  gives 5 steps or less. Thus, the scale does not move at each tick. The
  minimum span is 1 GB (`MEM_MIN_SPAN_GB`), so that noise does not look large.
  The limits show as small labels to the left of the chart, right-aligned to
  its left edge, one at the top and one at the bottom, so the line can never
  cover them. They share the left column with the big number without touching
  it: the number sits vertically between the two labels.
- **Storage:** The device keeps all 600 samples, in units of 0.01 GB, in two
  `array('H')` buffers (2.4 KB). In one buffer a gap is 0, and in the other it is
  0xFFFF, so that C-speed `max()` and `min()` on slices skip the gaps. The
  history is in RAM, so after a reset the chart fills again from the right.
- **Cost:** The device draws all 206 MEM columns again at each tick.

## 6. System architecture

```
┌──────────────────────── Mac (Apple Silicon) ────────────────────────┐
│                                                                     │
│  macmon pipe ──► JSON lines ──┐                                     │
│  (CPU, GPU, power)            │                                     │
│                               ├──► tach-sender.py ──► USB serial ───┼──┐
│  host_statistics64 ───────────┘    (1 line/0.5 s)                   │  │
│  (memory)                                                           │  │
└─────────────────────────────────────────────────────────────────────┘  │
                                                                         │
┌──────────────────────────── PyPortal ───────────────────────────────┐  │
│                                                                     │  │
│  usb_cdc.data ◄─────────────────────────────────────────────────────┼──┘
│       │                                                             │
│       ▼                                                             │
│  parse JSON ──► ring buffer for each metric ──► displayio redraw    │
│                                                                     │
└─────────────────────────────────────────────────────────────────────┘
```

### 6.1 Mac side: `tach-sender.py`

- Python 3 script. It runs as a normal user.
- It gets CPU, GPU and power data from
  [`macmon`](https://github.com/vladkens/macmon). `macmon pipe` writes one JSON
  object for each sample, and it does not need `sudo`.
- It gets memory data from `host_statistics64()` or `vm_stat`. See 5.1 for
  the definition.
- It finds the PyPortal data port. It must find the port automatically: look
  for the Adafruit USB vendor ID, group ports by board (USB serial number, or
  IORegistry location when macOS omits it), and select the *data* interface,
  not the console, by the higher device number.
- It sends one line each 0.5 s (`SEND_INTERVAL_S`).
- If the device disconnects, the script waits and connects again. It must not
  stop.
- A `launchd` agent starts it at login. `launchd/install.sh` installs the
  agent. If the script crashes, launchd restarts it (every 10s). If the script
  exits cleanly (e.g. missing `macmon` or `pyserial`), launchd does not restart it.

### 6.2 Device side: `code.py`

- `boot.py` enables the second USB serial channel:
  `usb_cdc.enable(console=True, data=True)`. The data channel carries only
  Tach messages, so the messages do not mix with the REPL console.
- Font failure never stops the device: any exception from loading the big font
  falls back to the scaled built-in font. The device then shows `NO FONT` at
  the top of the screen, to the right of the CPU label (the corner belongs to
  the label), because the console print is invisible
  while the console port stays closed (see README Troubleshooting).
- `code.py` reads lines from `usb_cdc.data`. It must not block. It reads the
  bytes that are available and keeps a partial line until it gets `\n`.
- CPU and GPU each have a ring buffer of 206 samples. MEM has a buffer of
  5 minutes (see 5.2).
- At each tick (0.5 s, `TICK_NS`), the device updates the big numbers and moves
  each chart by one column. The tick must agree with `SEND_INTERVAL_S` on the
  Mac. If no message arrived during a tick, the device uses the last value.
- The CPU and GPU charts are rings of bitmap columns. A scroll changes only
  which column shows at each position, and the device draws only the new
  column. The MEM chart is drawn again completely at each tick (see 5.2).
- Measured work for each tick, with a full MEM history: approximately 265 ms
  average, 278 ms maximum (at 170 chart columns). The 206-column layout redraws
  about 20 percent more per tick, so expect roughly 300–340 ms; still inside
  the 0.5 s tick. Re-measure with `TIMING` after the layout change. A 0.25 s
  tick is not possible.
- If there is no data for 5 seconds, the device shows that the connection is
  stale: the numbers become grey, and the charts show empty columns.

## 7. Message protocol

One JSON object on each line, ended by `\n`. Encoding: UTF-8.

```json
{"cpu":34.0,"gpu":88.0,"mem":97.4,"mem_total":128.0}
```

| Key | Unit | Range | Notes |
|---|---|---|---|
| `cpu` | % | 0–100 | Average of all cores. |
| `gpu` | % | 0–100 | GPU active residency from `macmon`. |
| `mem` | GB | 0–`mem_total` | Memory used (app + wired + compressed). Maximum value since the last message. See 5.1. |
| `mem_total` | GB | > 0 | Total memory. The device does not use it now (see 5.2). |

Rules:

- The device ignores keys that it does not know. Thus, the Mac script can send
  more data (for example power or memory pressure) before the device uses it.
- The device ignores a line that it cannot parse.
- A message must be shorter than 256 bytes.

## 8. Open questions

1. **Touch:** Use touch to change views (for example, a full-screen chart of
   one metric)? Not for the first version.
2. **Colour:** v0.1 gives each metric its own colour. Change the colour of the
   number when the value is high (for example, memory above 90%)?
3. **Brightness:** Make the backlight dim after a period with no data?
4. **Titano:** Make the layout scale from the screen size, or use a fixed
   layout for each board?

## 9. First version (v0.1)

1. `boot.py` and `code.py` on the PyPortal. Show 3 rows with test data.
2. `tach-sender.py` on the Mac. Send real data over `usb_cdc.data`.
3. Connect the two parts and look at the result.

Status: done. All 3 steps work. After v0.1:

- The update rate changed from 1 to 2 times each second.
- A `launchd` agent starts `tach-sender.py` at login. It restarts on crash
  (KeepAlive: SuccessfulExit=false), but exits cleanly when `macmon` or
  `pyserial` is missing so it does not crash-loop.
- The `mmap` question is resolved in 5.1: file-backed pages (including
  mmap'd models) are excluded by design, matching Activity Monitor.
