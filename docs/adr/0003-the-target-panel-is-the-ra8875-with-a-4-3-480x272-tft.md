# The target panel is the RA8875 with a 4.3" 480×272 TFT

Tach v2 drives a 4.3" 480×272 IPS TFT through an RA8875 display controller on its own SPI
breakout (Adafruit 1590 + 1591), in RGB565. We chose it over the physically nicer 3.5" bar-type
displays and over a bare RGB TTL panel, and we accept that the kit is out of stock at Adafruit
until late next week.

## Considered options

- **Bare RGB TTL panel with an ST7701S driver** (Adafruit 5805, 320×960 bar, 38 pins). Rejected:
  the ST7701S has no pixel RAM for our use, so the host must stream pixels at a continuous dot
  clock — a 192 KB frame buffer plus timed DMA, 40 signal wires, and a board designed for the
  RP2040/RP2350 instead of a general display link.
- **RA8875 with a 4.3" 480×272 TFT** (Adafruit 1590 + 1591). Chosen: the controller holds 768 KB
  of VRAM and executes line, rectangle, ellipse and scroll commands itself, so a 9-wire SPI link
  is the whole interface, and 480×272 gives four 68 px rows or three 90 px rows.
- **HX8357D 3.5" 320×480** (Adafruit 5846 / 2050). Runner-up: controller frame memory and
  240×320-class graphics over SPI in 16 wires, but no hardware scroll, so horizontal travel costs
  a copy of VRAM inside the controller.
- **Keep the SHARP 2.7" 400×240 memory display** (Adafruit 4694, owned). Rejected as the target:
  1 bit per pixel, no backlight, no colour, whole-frame updates. Kept as the development
  scaffold — [ADR-0004](0004-the-firmware-sends-drawing-operations-not-frames.md).

## Consequences

- We must write the RA8875 driver in Rust. No `ra8875` crate exists on crates.io; the reference
  is the datasheet plus the Arduino `Adafruit_RA8875` and MicroPython `ra8875` drivers.
- 480×272 is a different aspect ratio from 400×240, so layout must derive from panel dimensions.
- RGB565 makes anti-aliased glyphs possible, which changes how the numbers look from v1.
- The stock delay costs about a week of hardware time; the scaffold panel covers it.
- 1590 is sold as a board for 4.3" TFTs with an RA8875 — confirm the exact TFT mating and the
  backlight pin at first power-up.
