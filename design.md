# Tach — design

Status: v0.1 done, 2026-11-05. The display updates two times each second.

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

There are 3 rows. Each row is 80 pixels high. Each row has 3 parts:

```
|<-40->|<----- 110 ----->|<---------- 170 ---------->|
 CPU      78%              ╱╲__╱‾‾╲___╱╲
 small    big number       line chart, ~70 px high
 label    ~36 px high      170 px wide = ~85 s at 2 Hz
```

```
┌────────────────────────────────────────────┐
│ CPU   78%    ╱╲__╱‾‾╲___╱╲__                │
├────────────────────────────────────────────┤
│ GPU   92%    ___╱‾‾‾‾‾‾‾‾‾‾‾╲_              │
├────────────────────────────────────────────┤
│ MEM  97.4G   ‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾              │
└────────────────────────────────────────────┘
```

Rules:

- **Label:** 3 characters, small font.
- **Big number:** 5 characters maximum, for example `100%` or `97.4G`. A fixed
  width keeps the chart in the same position when the value changes. Use a
  smooth bitmap font, approximately 36 px high, with `adafruit_bitmap_font`. Do
  not use the scaled built-in `terminalio` font, because it is blocky.
- **Chart:** One pixel column for each sample. At 2 Hz, 170 columns show
  approximately 85 seconds of history. The chart scrolls from right to left.
  A faster rate gives less history: at 4 Hz, the chart shows only 42 seconds.
- **Memory unit:** Show GB, not MB. A Mac for LLMs can have 128 GB or more.
- **Chart scale:** CPU and GPU use 0–100%. Memory uses 0 to the total memory
  of the Mac. Thus, all 3 charts use a fixed scale, and the charts do not jump.

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

**Caution — memory-mapped model files:** Some LLM runtimes (for example
llama.cpp and Ollama with their default `mmap` setting) map the model file into
memory. These pages can show as "cached files", not as "memory used". Then the
chart does not show the full size of the model. MLX and runtimes with `mmap`
off load the weights into app or wired memory, and the chart shows them.
Test this with your runtimes during v0.1. If necessary, add "cached files" as
a second line on the MEM chart.

**Short spikes:** At 2 samples each second, the chart can miss a short memory
peak. Thus, the Mac script reads memory 4 times for each message (8 times each
second) and sends the maximum value.

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
  for the Adafruit USB vendor ID, and select the *data* interface, not the
  console.
- It sends one line each 0.5 s (`SEND_INTERVAL_S`).
- If the device disconnects, the script waits and connects again. It must not
  stop.
- A `launchd` agent starts it at login. `launchd/install.sh` installs the
  agent. If the script stops, launchd starts it again.

### 6.2 Device side: `code.py`

- `boot.py` enables the second USB serial channel:
  `usb_cdc.enable(console=True, data=True)`. The data channel carries only
  Tach messages, so the messages do not mix with the REPL console.
- `code.py` reads lines from `usb_cdc.data`. It must not block. It reads the
  bytes that are available and keeps a partial line until it gets `\n`.
- Each metric has a ring buffer of 170 samples.
- At each tick (0.5 s, `TICK_NS`), the device updates the big numbers and moves
  each chart by one column. The tick must agree with `SEND_INTERVAL_S` on the
  Mac. If no message arrived during a tick, the device uses the last value.
- Each chart is a ring of bitmap columns. A scroll changes only which column
  shows at each position, and the device draws only the new column.
- Measured work for each tick: approximately 145 ms average, 192 ms maximum.
  Thus, a 0.25 s tick is possible, but it has little margin.
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
| `mem_total` | GB | > 0 | Total memory. Used for the chart scale. |

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
- A `launchd` agent starts `tach-sender.py` at login.
- Still to do: the `mmap` test in 5.1.
