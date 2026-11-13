# Wiring — Pico 2 to the 2.7" SHARP Memory Display

**Board:** Raspberry Pi Pico 2 (RP2350). **Not** the Pico 2 W: the WM820Wi-Fi chip and its
antenna are dead weight here, and the Wi-Fi coprocessor takes SPI and UART pins we want to keep
free. Everything else is identical.

**Panel:** Adafruit 2.7" 400×240 SHARP Memory Display breakout (PID 4694), 9 pins on one edge:
`EXTIN DISP EXTHD CS MOSI SCLK GND 3V3 VIN`.

## Signal wiring

| Pico 2 pin | To panel pin | Why |
| --- | --- | --- |
| **GP18** | `SCLK` | SPI0 SCK. Verified valid in `rp235x-hal` 0.4 (`spi/pins.rs`: SPI0 SCK = GP2, GP6, GP18, GP22). |
| **GP19** | `MOSI` | SPI0 TX. Verified (SPI0 TX = GP3, GP7, GP19, GP23). |
| **GP17** | `CS` | Driven as **plain GPIO**, not hardware SPI CS. The panel's CS is **active high** — the board silks says so — and hardware CS on RP2350 is active low, so software control is the honest way. |
| **GP16** | `DISP` | Panel enable, as GPIO. High = panel on. |
| **VBUS** | `VIN` | 5 V from USB, inside the panel board's 3.5–5 V range. |
| **GND** | `GND` | Common ground. Do this first. |

Leave unconnected:

- `3V3` on the panel board — power goes in at `VIN`, so do not tie two 3.3 V rails together.
- `EXTIN` and `EXTHD` — external VCOM and hold-off modes. The Rust reference driver, the Arduino
  library and the CircuitPython driver all run the panel without them.

No MISO wire: the panel takes data only.

## Keep these free for the debugger

GP20 = SWDIO, GP21 = SWCLK on the RP2350. That is why CS sits on GP17 and not GP21, and why no
SPI0 function lands on GP20. The RPi debug probe takes SWDIO, SWCLK, GND and VTREF only.

## One check before power

The board's `3V3` pin is either the output of an onboard regulator or a pass-through for the logic
rail, and the guide does not say. With `VIN` powered from USB and nothing else connected, measure
`3V3` to `GND`:

- **about 3.3 V** — the board makes it. Leave the pin alone, as the table says.
- **0 V or 5 V** — stop and report it before connecting the data lines.

## SPI settings

| Setting | Value | Source |
| --- | --- | --- |
| Mode | CPOL 0, CPHA 1 (`MODE_1`) | `sharp-memory-display` 0.3: `Polarity::IdleLow`, `Phase::CaptureOnSecondTransition` |
| Clock | start at **12 MHz** | 400×240 at 1 bpp needs 0.96 Mbit per whole frame; 12 MHz moves it in 8 ms |
| Bit order | **MSB first** on the wire, LSB first per pixel | the panel wants pixel 0 of a byte clocked first |
| CS | high while clocking, low between transactions | board silks: "CS is active high" |
