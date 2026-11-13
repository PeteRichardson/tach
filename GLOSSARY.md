# Tach

Tach is a small display that shows the live processor, graphics and memory load
of a Mac over one USB cable.

## Language

**Sample**:
One measurement of one metric on the Mac.
_Avoid_: reading, data point

**Message**:
One line from the Mac that carries the newest sample of each metric.
_Avoid_: packet, frame, payload

**Frame**:
One update of what the display shows. Frames per second sets how smooth motion
looks; samples per second sets how fast history arrives.
_Avoid_: tick, refresh (as a noun)

**Refresh**:
The transfer of the changed pixels from the device to the panel.
_Avoid_: draw, flush

**Draw**:
The work of deciding which pixels change in a frame.
_Avoid_: render

**Sample interval**:
The time between two messages from the Mac.

**Frame interval**:
The time between two frames on the display. The sample interval and the frame
interval are separate numbers with separate costs.
**Tick**:
Avoid. In this project it has meant the sample interval and the frame interval
at the same time.

**Scroll**:
The movement of chart history one column to the left when a new sample arrives.
Some panels shift their own image sideways on command; others need the host to redraw
every pixel of the chart.
_Avoid_: pan, slide

**Column**:
One pixel column of a chart. A CPU or GPU column holds one sample; a memory
column holds the lowest and highest sample of its period.
_Avoid_: bucket, bin

**Frame buffer**:
The block of memory that holds the picture the panel shows. It lives either in the host
chip or in the display driver chip, and which one it is decides what the host may send.
_Avoid_: video RAM, VRAM

**Partial update**:
Sending only the pixels that changed, instead of a whole frame.
_Avoid_: dirty region, blit

**Stale**:
A metric whose newest sample is older than the staleness limit. The display
greys a stale value so that a dead source cannot look live.
_Avoid_: offline, frozen, lost

**Peak keep**:
The rule that a memory peak which ever arrived must still appear in the chart,
whatever smoothing or animation the display applies.
_Avoid_: smoothing (for this rule)

**Motion smoothness**:
How evenly the eye sees the chart scroll. Set by frames per second and by the
evenness of their timing, not by the size of the values.
_Avoid_: smoothness (unqualified), fluidity

**Freshness**:
How often a metric's value actually changes, as opposed to how often we ask for it. A fast poll of
a stale counter is not a fresh sample: the public GPU registry counter changes about 6 times a
minute under load, while macmon's GPU ratio changes about 6 times a second.
_Avoid_: rate (means poll rate), real-time

**Retention nudge**:
A periodic one-pixel shift of the whole screen, so that permanently lit pixels
do not age faster than their neighbours.
_Avoid_: pixel jiggle

**Operation**:
One drawing instruction issued to a panel backend: scroll, line, rectangle, ellipse,
glyph, clear, brightness. A tick of the chart is one scroll operation, not a redraw.
_Avoid_: command (reserved for controller register writes), primitive

**Shim**:
The temporary path that turns each operation into writes on a host frame buffer, so the
firmware can run on the development panel until the target controller arrives.
_Avoid_: emulation, fallback

**Level**:
One of the four semantic roles an on-screen element carries: background, dim, line, grid.
Each panel maps the four levels onto its own colours, which is how one layout looks right
on a colour panel and on a one-bit panel with no grey.
_Avoid_: layer, style, theme

**Pull**:
The device asking the Mac for one sample per frame and drawing the reply. The alternative, push,
lets the Mac's timer decide when the display moves.
_Avoid_: request, poll (poll is the CPU-side read)

**Sub-sample**:
One of the lowest, highest and newest memory readings taken inside a single message period.
Carried so that a short spike still reaches the chart when messages arrive slowly.
_Avoid_: aggregate, bucket value

**Glyph atlas**:
Pre-rendered bitmaps of every character at a fixed size and weight, baked into firmware at
build time from the source font, and blitted as small rectangles.
_Avoid_: font file, bitmap font (the v1 loader that reads a .bdf over USB)

**Scaffold**:
Hardware used to develop against until the target hardware arrives. The 2.7 inch one-bit
SHARP panel is the scaffold; the 4.3 inch colour panel is the target.
_Avoid_: prototype (used for throwaway code, not hardware)

## Decisions

1. [ADR-0001](adr/0001-rp2350-and-rust-for-the-device-firmware.md) — RP2350 and Rust for the device firmware
2. [ADR-0002](adr/0002-chart-columns-are-spans-of-time.md) — Chart columns are spans of time
3. [ADR-0003](adr/0003-the-target-panel-is-the-ra8875-with-a-4-3-480x272-tft.md) — The target panel is the RA8875 with a 4.3" 480×272 TFT
4. [ADR-0004](adr/0004-the-firmware-sends-drawing-operations-not-frames.md) — The firmware sends drawing operations, not frames
5. [ADR-0005](adr/0005-the-mac-clock-times-the-chart-and-the-rate-goes-on-the-wire.md) — The Mac clock times the chart, and the rate goes on the wire
6. [ADR-0006](adr/0006-the-device-pulls-one-sample-per-frame.md) — The device pulls one sample per frame
7. [ADR-0007](adr/0007-cpu-from-mach-ticks-macmon-stays-for-gpu-only.md) — CPU from Mach ticks, macmon stays for GPU only

Working brief and open questions: [docs/device-v2.md](device-v2.md).
