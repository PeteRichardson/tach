# Tach device v2 — RA8875 display, RP2350, smooth motion

Status: **in design**. Replaces the PyPortal path in [`../design.md`](../design.md), which stays
as the record of v1. Terms: [`../GLOSSARY.md`](../GLOSSARY.md). Decisions:
[`adr/`](adr/). This file is the working brief for the firmware in [`../device/`](../device/).
Bring-up status: [`../device/BRINGUP.md`](../device/BRINGUP.md).

## The problem v2 solves

v1 redraws the whole screen per message, synchronously, and one frame costs 190–340 ms on the
PyPortal ([measured, ADR-0001](adr/0001-rp2350-and-rust-for-the-device-firmware.md)). The
display is therefore a slideshow at 2 Hz and cannot be smoother, in any language. v2 targets
continuous chart travel at about 30 frames per second.

## Hardware

| Part | Choice | Notes |
| --- | --- | --- |
| Computer | Mac, Apple Silicon | Bus-powered, one cable, no Wi-Fi, no `sudo`. |
| MCU | Raspberry Pi Pico 2 (RP2350, Cortex-M33) | 520 KB RAM, 4 MB flash, PIO, USB. |
| Target panel | RA8875 driver board + 4.3" 480×272 IPS, RGB565 | Adafruit 1590 + 1591. **Stock delay: arrives late next week.** |
| Scaffold panel | 2.7" 400×240 SHARP Memory Display (Adafruit 4694) | Owned. Monochrome, no backlight, black-on-grey. Usable today. |
| SWD probe | Existing RPi debug probe + `probe-rs` | Flashing and live RAM inspection without a serial print in the frame loop. |

Panel rule, learned the hard way: the panel must have **frame memory at the controller** or an
equivalent self-refresh. A bare RGB TTL panel (Adafruit 5805, ST7701S) needs a dot-clock pixel
stream and a 192 KB continuous frame buffer, so it is out — see
[ADR-0003](adr/0003-the-target-panel-is-the-ra8875-with-a-4-3-480x272-tft.md).

SHARP breakout pins (`EXTIN DISP EXTHD CS MOSI SCLK GND 3V3 VIN`), board note: *"3 or 5V logic,
CS is active high"*. Needed wires: SPI SCK/MOSI + CS + DISP. `EXTIN`/`EXTHD` can be tied at the
board; verify against the datasheet at bring-up.

The board's only IC is a Texas Instruments **SN74HC4050** (`HC4050M G4`), a hex non-inverting
buffer used as a level shifter — hence the 3 V/5 V logic note. Consequences: the RP2350's 3.3 V
GPIO connects directly, the active-high CS comes from the SHARP protocol rather than from any
inversion on this board, and the panel's own driver sits on the COF under the glass. No SHARP
orderable part code is readable on the board, so the panel revision stays unconfirmed by sight. The
firmware's five-pattern self-test confirmed the 400×240 geometry on the glass instead (2026-11-12,
[`../device/BRINGUP.md`](../device/BRINGUP.md)).

## Firmware shape

`tach-sender.py` stays. It is the Mac side, it runs under launchd, and it owns three things it
should own: `macmon`'s lifecycle, the measurement rate, and the wire format.

```
tach-sender.py  ── one JSON line per tick over USB CDC data ──▶  device/ (Rust, no_std)
   ~2 Hz, unchanged rate                                       frame loop ~30 Hz
```

Four layers inside the device:

| Layer | Holds | Does |
| --- | --- | --- |
| Panel driver | RA8875 registers, or the SHARP crate | `Scroll`, `Line`, `Rect`, `Ellipse`, `Glyph`, `Clear`, `Brightness` |
| Layout | Geometry derived from panel width/height | Rows, chart window, number slot, axis, in panel pixels |
| Chart model | Column spans of time, peak keep, y-axis zoom ([ADR-0002](adr/0002-chart-columns-are-spans-of-time.md)) | Position per sample, no invention of peaks |
| Wire reader | Rate + Mac clock stamps | Latest-value replace, memory sub-samples, gap detection |

Layout derives from the panel's size instead of the hard-coded 320×240 that made issue #36
("only supports 320x240 panel") true. Same numbers, then 400×240, then 480×272, no rewrite.

## Four drawing levels

The image is composed of four semantic levels, mapped per panel because a mono panel has no grey:

| Level | RA8875 (RGB565) | SHARP scaffold (1 bpp) |
| --- | --- | --- |
| background | near-black | light grey |
| dim | dark grey | light grey |
| line | colour per row | dark |
| grid | darker colour | light, dotted/checked (dither cannot be simulated) |

## Transport

- RA8875 is driven by **commands against its own 768 KB VRAM**, never by pushed frames:
  full-frame RGB565 at 30 fps would need 7.8 Mbyte/s and the reference driver runs SPI at
  12 MHz. See [ADR-0004](adr/0004-the-firmware-sends-drawing-operations-not-frames.md).
- Horizontal travel redraws each moving curve once per frame from the stored samples, at an exact
  fractional x offset. Hardware scroll is **not** the travel path — see Motion budget below.
- Glyphs come from an atlas baked from `fonts/tach-36.bdf` (digits) and `fonts/tach-16.bdf`
  (row labels) at build time, blitted as small rectangles. No font files on the device at
  runtime. The BDF files are rendered from Monaspace Neon SemiBold by `fonts/make-fonts.py`.
- The SHARP scaffold has no VRAM and no scroll: the shim translates each operation into writes on
  a 12 KB frame buffer in MCU RAM and one flush per frame.
- Brightness is PWM on the RA8875 backlight pin.

## Time base

[ADR-0005](adr/0005-the-mac-clock-times-the-chart-and-the-rate-goes-on-the-wire.md). The Mac
stamps every message with its monotonic clock and announces the sample rate once at connect.
Columns are spans of time, so a 2 Hz stream can drive 30 Hz motion, and a dead `macmon` can no
longer fake a live number (issue #1) because a stale column simply stops advancing and the
staleness rule blanks the panel (issue #27 becomes one rule in one place).

## Motion budget

The arithmetic that decides what "smooth" can mean here.

```
travel = chart_width_px / window_seconds     (px per second)
smooth needs travel >= 1 px per frame = 30 px/s at 30 fps
```

v1 windows against a 286 px chart (400 px panel minus the number slot):

| Row | Window v1 | Travel | One pixel step every |
| --- | --- | --- | --- |
| CPU / GPU | 100 s | 2.9 px/s | 10 frames |
| Memory | 300 s | 0.95 px/s | 31 frames |

So the v1 chart cannot look smooth at whole-pixel placement: it moves one pixel every third of a
second. Time-positioned columns ([ADR-0002](adr/0002-chart-columns-are-spans-of-time.md)) fix
*placement* and honesty, not apparent motion. The levers left are placement precision, speed and
width — see Settled parameters below.

### Hardware scroll is out as the travel path

Read from the reference driver (`Adafruit_RA8875.cpp`):

- `scrollX(dist)` writes registers `0x24`/`0x25` — a horizontal **scroll start position**. It is a
  read offset, not a blit: content wraps inside the window, nothing is copied or erased.
- Consequence 1: that offset is one register, so **all rows share one travel rate**. Our rows want
  different rates (CPU/GPU fast, memory compressed), so scroll cannot serve them all.
- Consequence 2: the offset is whole pixels. A rate that is not an exact multiple of the frame rate
  must be rounded, so the chart advances 1 px on some frames and 0 on others — a 33 ms position
  error, which is the jitter we are trying to remove.
- `setScrollWindow(x, y, w, h, mode)` sets the window edges with `0x38`–`0x3f` and the scroll
  function with `0x52` (`BOTH` 0x00, `LAYER1` 0x40, `LAYER2` 0x80, `BUFFER` 0xC0).

Scroll stays useful for the retention nudge (whole-layer integer shift, no redraw) and for a
single uniform-rate row.

### Travel path we use instead

Redraw each moving curve once per frame from the sample store, at the exact fractional x offset,
with the RA8875 geometric engine (about 8 bytes per point):

```
3 rows x 280 segments x 8 B x 30 fps = 202 KB/s   (SPI at 12 MHz carries about 1.5 MB/s)
```

About 13 % of the link. It removes the shared-rate limit, keeps sub-pixel placement exact, and
draws the curve straight from the sample store — which is what issue #9 needs for peak honesty.

The cost is a curve redraw every frame even when no new sample arrived, which the scaffold must
also do so that what we measure on the SHARP predicts the RA8875.

### Settled parameters (Q26 smooth-first, Q27 8 Hz)

Sub-pixel placement relaxes the smoothness rule. The `1 px per frame` line above only binds a panel
that must land pixels on whole pixels. Because we draw anti-aliased segments at a fractional offset,
the eye follows motion down to about 0.3 px per frame, provided the displacement is even. The limit
is no longer speed alone; it is variance in displacement.

Fix the speed from the sample spacing, so the curve stays dense enough to read, and derive the
history from the width:

```
speed   = 1.5 px per sample x 8 samples per second = 12 px/s = 0.4 px per frame at 30 fps
window  = chart_width_px / speed
```

| Panel | Chart width | CPU/GPU window | px per frame | px per sample |
| --- | --- | --- | --- | --- |
| SHARP 400×240 (scaffold) | 302 px | 25 s | 0.40 | 1.5 |
| RA8875 480×272 (target) | 382 px | 32 s | 0.40 | 1.5 |

The dial, same width, for reference:

| Window | Travel | px per frame | Verdict |
| --- | --- | --- | --- |
| 10 s | 30 px/s | 1.00 | crisp, but only 10 s of context |
| 25 s | 12 px/s | 0.40 | **chosen** — smooth, readable, 200 samples |
| 50 s | 6 px/s | 0.20 | marginal, motion reads as slow drift |
| 100 s (v1) | 3 px/s | 0.10 | reads as frozen between steps |

Memory keeps its compressed 5-minute axis, with the right edge travelling at the same 12 px/s as
CPU and GPU. One speed across the whole screen, and the 5 minutes of context the display exists
for. Column content stays min/max per column, so peaks survive the compression.

Other numbers that follow: three rows of 80 px (scaffold) and 90 px (target); number slot 90 px;
margins 8 px; grid tick every 5 s, which lands 60 px apart at this speed.

Sample store: CPU and GPU 200 samples each, memory 2400 samples at 8 Hz for 300 s — about 20 KB of
the RP2350's 520 KB RAM. Not a constraint.

## Wire

**The device pulls once per frame** ([ADR-0006](adr/0006-the-device-pulls-one-sample-per-frame.md)).
The Mac answers with every metric it holds plus the arrival time of each, so the two sides have no
rate to agree on and the frame grid stays even because the device owns the clock.

The request is one line, `{"pull":N}`, with N counting up from 1 for each pull. The reply carries
the same `"pull":N`, so the device can tell a reply to this pull from a late reply to an earlier
one, and drop the late one.

**Sources are split** ([ADR-0007](adr/0007-cpu-from-mach-ticks-macmon-stays-for-gpu-only.md)): CPU
ticks read natively at the frame rate, memory read natively, `macmon` kept for GPU alone at about
9 Hz. Three freshness classes share one reply, and each sample carries its own stamp — which is
fine, because columns are spans of time.

`macmon` is the rate limit for GPU only. Measured on this Mac: `macmon json -i 33` gives 8.5–9.3
lines per second, `gpu_active_ratio` changes about 6.5 times per second, `host_statistics` costs
1.3 µs, `host_statistics64` costs about 1 µs. Python reached 26.5 Hz when asked for 30 Hz, so the
language is not the limit. Full table in Mac-side sources below.

`host_statistics64` keeps the Activity Monitor memory formula and adds min/max/last sub-samples per
reply, so short memory spikes survive a compressed chart column (issue #9).

## Mac-side sources (measured, 2026-XX, M5 Ultra, 36 cores, macOS)

Question: can the Mac feed faster than `macmon` does? Measured with ctypes against the same system
APIs (`/tmp/tach-smooth/rate_probe.py`, `freshness.py`, `compare.py`). Poll rate is not the same
thing as data freshness, so both were measured.

| Source | Cost per read | Ceiling | Value actually changes at | Verdict |
| --- | --- | --- | --- | --- |
| `host_statistics(HOST_CPU_LOAD_INFO)` — CPU ticks, host-wide | **1.3 µs** | ~700 kHz | ~~every poll~~ **about 1 Hz** on macOS 27 (re-measured 2026-11-12) | too stale |
| `host_processor_info(PROCESSOR_CPU_LOAD_INFO)` — CPU ticks, per core | 22 µs from Python | ~45 kHz | every few ms; ~133 ticks per 33 ms window over 36 cores | **use this** |
| `host_statistics64(HOST_VM_INFO64)` — memory | **1 µs** | ~1 MHz | ~10 Hz (page counts move constantly) | already native, keep |
| IORegistry `IOAccelerator` → `PerformanceStatistics` | 36 µs | ~28 kHz | **~0.3 Hz** | too stale for GPU |
| `macmon pipe -i 33` | subprocess, ~29 keys | **8.5–9.3 Hz** | ~6.5 Hz for `gpu_active_ratio` | **only GPU source that is fresh** |

Surprises:

- **CPU was never the problem.** Ticks run at ~3.5 kHz over 36 cores (about 0.28 ms per tick), so
  a 33 ms window holds ~118 ticks — enough resolution for a percent readout. v1 gets CPU from
  `macmon` and is therefore capped at macmon's rate for no reason.
- **Correction, 2026-11-12: read the per-core counters.** On macOS 27 the host-wide sum from
  `host_statistics(HOST_CPU_LOAD_INFO)` stayed the same for about 1 s at a time, at idle and with
  one core busy, and then moved in one burst. The per-core counters from `host_processor_info`
  moved 1192 times in 3 s with no gap over 3 ms. Over 33 ms windows at idle they gave a median of
  2.2 %, against 2.5 % pooled over the same 2 s, so short windows are not biased.
- **The public GPU counter is worse than macmon, not better.** `IOAccelerator`'s
  `PerformanceStatistics` has `Device Utilization %` and `Renderer Utilization %`, reads in 36 µs, and
  sits under 28 kHz — but while GPU work was running the value changed only 4 times in 12 seconds,
  mostly stuck at 0 then 90. `macmon`'s `gpu_active_ratio` changed ~6.5 times per second. macmon gets
  its GPU number from the private `IOReport` API; the registry mirror is not the same thing.
- **Memory changes at about 10 Hz, not 30.** Page counts do not move every 33 ms. Time placement
  (ADR-0002) makes this harmless: a held sample is honest, a fabricated one is not.
- Python reached 26.5 Hz when asked for 30 Hz. The language was never the rate limit either.

### Consequence

Drop `macmon` for CPU. Read ticks natively at the frame rate. Keep the `macmon` subprocess for GPU
only, at its own best rate (about 9 Hz), and time-place its samples like every other sample. The
sender then holds three different freshness classes in one message and stamps each with its own
arrival time — which the device already handles, because columns are spans of time.

A Rust replacement for `macmon` would not help: to beat it on GPU we would have to reimplement the
undocumented `IOReport` API ourselves, which is the fragile part macmon already owns. A Rust rewrite
of the *sender* is a different question — dependency removal, not rate.

CPU semantics change with this: macmon's `cpu_active_ratio` versus the tick ratio
`busy / (busy + idle)` across all cores, which is what Activity Monitor shows. Observed side by side
they are close (native 1.4–4.8 %, macmon 1.5–3.0 %) but they are not the same number.

## Open frontier

Only one thing is still unknown, and it needs hardware in hand.

1. **RA8875 SPI clock** — the reference driver uses 12 MHz; the datasheet maximum and the real limit
   with jumper wires are both unknown. Blocks the frame budget arithmetic on the target panel.
2. Closed, 2026-11-12: **pull round trip**. Measured on the scaffold over USB CDC full speed: average
   0.49 ms, worst 0.87 ms in 1789 pulls at 30 fps, no reply missed. It fits the frame budget.

Everything else below is settled. Change one and say so, because the layout constants and the wire
format both follow from these.

## Settled parameters

| Item | Value | Why |
| --- | --- | --- |
| MCU | RP2350, Rust, `no_std` | [ADR-0001](adr/0001-rp2350-and-rust-for-the-device-firmware.md) |
| Target panel | RA8875 + 4.3" 480×272, RGB565 | [ADR-0003](adr/0003-the-target-panel-is-the-ra8875-with-a-4-3-480x272-tft.md) |
| Scaffold panel | SHARP 2.7" 400×240 mono, via the frame-buffer shim | [ADR-0004](adr/0004-the-firmware-sends-drawing-operations-not-frames.md) |
| Travel path | Per-frame curve redraw at fractional x, not hardware scroll | Motion budget above |
| Travel speed | 12 px/s (0.4 px per frame at 30 fps) | smooth at 1.5 px per sample at 8 Hz |
| Windows | Derived: width ÷ 12 px/s. 25 s scaffold, 32 s target | smooth first (Q26) |
| Memory row | Compressed 5-minute axis, right edge at 12 px/s, min/max per column | keeps the context the device exists for |
| Number slot | 114 px, digits 36 px (Q34) | chart width is not worth 2 s of history |
| Rows | 3, at 80 px scaffold / 90 px target (Q37) | bigger beats more |
| Size scaling | Proportional to panel height on both panels, no scaler (Q30) | a scaled font lies about legibility |
| Bold digits | RGB565 only; no fake bold on mono (Q35) | 1 bpp cannot show the two-tone edge |
| Peak label | None; the chart is enough (Q29) | user request |
| Cadence | Device pulls one sample per frame (Q33) | [ADR-0006](adr/0006-the-device-pulls-one-sample-per-frame.md) |
| Sources | CPU native ticks at frame rate, memory native, macmon for GPU only (Q31) | [ADR-0007](adr/0007-cpu-from-mach-ticks-macmon-stays-for-gpu-only.md) |
| CPU meaning | `busy/(busy+idle)` over all cores, the Activity Monitor definition (Q32) | consistent with memory |
| Sampling | ~8 Hz from macmon, native calls far faster, time-placed regardless | ADR-0002 |
| Retention nudge | 1 px every 5 min, 4 diagonal positions, as one whole-layer scroll (Q39) | free on the controller |
| Sender language | Keep Python 3 + pyserial (Q38) | measured 26.5 Hz against a 30 Hz request; Python was never the limit |
| Wire keys | `pull, v, t, hz, cpu, tc, gpu, tg, mem, mem_lo, mem_hi, tm, mem_total, ok` (Q36) | unknown keys ignored, as v1 promises; `pull` echoes the request so a late reply is not taken for this one |
| Issue triage | Deferred until the new device boots; PyPortal-only issues close as superseded (Q40) | avoids closing issues against hardware still in service |
| v1 archive | CircuitPython code to `legacy/pyportal/` at retirement (Q40) | keeps the repo root for v2 |

## Build steps

Done, 2026-11-12. Details and evidence: [`../device/BRINGUP.md`](../device/BRINGUP.md).

1. Toolchain: `rustup target add thumbv8m.main-none-eabihf`, `brew install picotool probe-rs-tools`.
   Homebrew's `cargo` hides rustup's, so build with `PATH="$HOME/.cargo/bin:$PATH"`.
2. `device/` cargo project on `rp235x-hal` 0.4, flashed and read over RTT with `probe-rs` through
   the RPi debug probe. No `embedded-graphics` or `sharp-memory-display`: the SHARP driver
   (`device/src/sharp.rs`) and the 1-bit frame buffer are our own, so they implement the ADR-0004
   operations directly. The image needs an `IMAGE_DEF` block, or the RP2350 boot ROM does not
   start it.
3. SHARP panel bring-up: the five-pattern self-test shows the 400×240 geometry, the bit order and
   CS polarity are correct on the glass.
4. Operations on the SHARP frame buffer, with chart motion from fake 8 Hz samples: 29–30 fps,
   worst draw 2.0 ms, worst present 7.0 ms for 9362 B at 10.71 MHz SPI (12 MHz requested).

Next:

5. Done: present now runs at wire speed (7.0 ms for 9362 B at 10.71 MHz). The HAL's
   `SpiBus::write` waited after each byte; `device/src/fifo_spi.rs` keeps the FIFO full.
6. Done: USB CDC on the device (`device/src/link.rs`, stack in the USB interrupt). One pull per
   frame round trips in 0.49 ms average, 0.87 ms worst, with a test responder on the Mac. The reply
   is read (`device/src/wire.rs`) into the chart model (`device/src/chart.rs`): one bucket per
   pixel of travel, each with low, high and newest value, placed by its Mac stamp. Every curve is
   redrawn from the model on every frame. The staleness rule (`chart::STALE_MS`, 5 s) is in: a row
   whose newest Mac stamp is older than 5 s is stale, so a dead macmon stops the GPU row while CPU
   and memory go on (issue #1). No reply for 5 s blanks every plot instead of moving old curves
   (ADR-0005, ADR-0006). The number slot (`device/src/text.rs`) shows the row label and the
   number in the v1 formats ("7%", "97.4G"), and "--" for a stale row. Its glyphs are baked at
   build time from `fonts/tach-36.bdf` and `fonts/tach-16.bdf`, which `fonts/make-fonts.py`
   renders from Monaspace Neon SemiBold (OFL, `fonts/OFL.txt`). The slot costs about 1.4 ms of
   draw per frame. The memory row has its 5-minute compressed axis (`chart::Axis`,
   `chart::draw_compressed`): the right edge travels at 12 px/s like CPU and GPU, the left edge
   is 300 s old, and each column is a min/max stroke over its time span, so a short peak stays
   visible. Memory keeps 3615 buckets of 83 ms (about 87 KB). Marks at 1, 2, 3 and 4 minutes stay
   in place. With a full 5-minute history, draw takes about 8.2 ms per frame (3.8 ms before),
   and the panel stays at 29–30 fps. Not yet: the zoomed memory y axis of v1 (design.md 5.2).
7. Started: `tach-sender.py` answers each `{"pull":N}` with the Q36 keys and the same `pull` value,
   `ok: false` when macmon is stale. CPU from per-core Mach ticks, memory low/high since the last
   reply, GPU from macmon at 10 Hz while the v2 device is connected. A thread serves the v2 device
   beside the v1 push loop, so the PyPortal keeps working. Measured with the scaffold: 0.41 ms
   average round trip, no miss in 20 s.
8. When the RA8875 kit lands: port the constants to 480×272, write the RA8875 driver, measure the
   real SPI ceiling.
