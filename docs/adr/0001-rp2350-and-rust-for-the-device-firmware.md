# Build the device firmware on RP2350 in Rust

The PyPortal cannot show smooth motion: its panel is wired with one data line, and the
measured transport is 0.4 Mpixel/s, which caps a three-row frame at 2.9 frames per second
(5.2 frames per second if all drawing cost were removed). `macmon` also caps CPU and GPU at
about 8 samples per second, so no language change on that board fixes the problem. We
decided to build the next device on a Raspberry Pi Pico 2 (RP2350) in Rust, bus-powered from
the Mac over native USB, keeping the existing one-cable behaviour and the JSON-lines
protocol. The PyPortal keeps running as it is today until the new device is proven.

## Considered options

- **Optimise CircuitPython on the PyPortal**: capped near 5–7 frames per second by the
  1-bit write bus. Rejected: still visibly stepped, and the bus cannot be changed.
- **Rust on the same PyPortal board**: uncertain gain on the same 1-bit bus after weeks of
  driver work. Rejected: the ceiling is physical.
- **Raspberry Pi Zero 2 W with Linux**: any frame rate, but data needs a USB Ethernet
  gadget, and the project inherits a Linux image, an updater and macOS network
  configuration. Rejected for now: more operational weight than the display needs.

## Consequences

`circup`, drag-and-drop `code.py`, and runtime loading of `/fonts/tach-36.bdf` are gone.
Glyph bitmaps are generated at build time from that same BDF file instead, which also makes
anti-aliased numbers possible. `tach-sender.py` keeps its protocol and gains a rate field.
