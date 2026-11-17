# Wiring — Pico 2 to the RA8875 driver board and 4.3" TFT

**Board:** Raspberry Pi Pico 2 (RP2350). **Not** the Pico 2 W: the WM820Wi-Fi chip and its
antenna are dead weight here, and the Wi-Fi coprocessor takes SPI and UART pins we want to keep
free. Everything else is identical.

**Controller:** Adafruit RA8875 Driver Board for 40-pin TFT Touch Displays (PID 1590), **Rev E**
(tristate circuit on MISO, new backlight driver). 15 pins on one edge:
`VIN GND 3V0 LITE SCK MISO MOSI CS RST WAIT INT Y+ Y- X- X+`.

**Panel:** 4.3" 480×272 40-pin TFT with resistive touch, on the board's FPC connector.

Source for the pin facts below:
[Adafruit RA8875 guide](https://learn.adafruit.com/ra8875-touch-display-driver-board?view=all).

## Signal wiring

| Pico 2 pin (physical) | To board pin | Why |
| --- | --- | --- |
| **GND** (38) | `GND` | Common ground. Do this first. |
| **VBUS** (40) | `VIN` | 5 V from USB, inside the board's 3–5 V range. The board makes its own 3.3 V and drives the backlight booster from `VIN`. |
| **GP18** (24) | `SCK` | SPI0 SCK. Verified valid in `rp235x-hal` 0.4 (`spi/pins.rs`: SPI0 SCK = GP2, GP6, GP18, GP22). |
| **GP19** (25) | `MOSI` | SPI0 TX. Verified (SPI0 TX = GP3, GP7, GP19, GP23). |
| **GP16** (21) | `MISO` | SPI0 RX. Verified (SPI0 RX = GP0, GP4, GP16, GP20). The firmware reads status registers and touch data, so this wire is necessary. |
| **GP17** (22) | `CS` | Driven as **plain GPIO**. Active **low** on this board. |
| **GP15** (20) | `RST` | Active low: pull low to reset the RA8875. |
| **GP14** (19) | `INT` | Goes low when the panel is touched. Optional, but the guide says it improves touch accuracy. |
| **GP13** (17) | `WAIT` | Optional. Active low while the RA8875 is busy, so the firmware can poll a pin and not the status register. |

Add one resistor:

- **10 kΩ from `CS` to 3V3(OUT) (36).** The guide: *"This pin is not pulled high by default!"*
  Without the pull-up, the chip is selected while the Pico boots or resets. If you do not fit the
  resistor, the firmware must drive GP17 high before anything else.

Leave unconnected:

- `3V0` — the output of the board's own regulator (at least 100 mA). Do not tie it to the Pico
  3V3: two 3.3 V rails must not meet.
- `LITE` — an external PWM input for the backlight. The firmware dims the backlight with the
  RA8875's own PWM1 output, as the Adafruit library does. The guide does not say if `LITE` is
  pulled up. If the backlight stays dark after the firmware enables PWM1, wire `LITE` to a free GPIO
  and record the result here.
- `Y+ Y- X- X+` — these are for an external touch controller (TSC2007, STMPE610). The 40-pin FPC
  already carries the touch lines to the RA8875's own touch controller.

## Keep these free for the debugger

GP20 = SWDIO, GP21 = SWCLK on the RP2350. That is why no SPI0 function lands on GP20, although it
is a valid SPI0 RX pin. The RPi debug probe takes SWDIO, SWCLK, GND and VTREF only.

## Checks before power

1. **FPC:** open the latch, put in the panel ribbon with the copper contacts facing **away from**
   the board, close the latch. Do this with the power off. Verified on the glass 2026-11-16: with
   the contacts toward the board, the RA8875 reads and writes every register correctly, but the
   panel stays fully dark, with no backlight and no image.
2. **Backlight jumpers:** leave them at the default. The booster gives 25 mA or 50 mA at up to
   24 V, and the guide warns that the wrong panel can be damaged. 25 mA is the setting for a 4.3"
   panel; compare it with the panel's datasheet.
3. **3V0:** with `VIN` powered from USB and nothing else connected, measure `3V0` to `GND`. It must
   be about 3.3 V. If it is 0 V or 5 V, stop and report it before you connect the data lines.

Logic levels need no work: the board has a CD74HC4050 level shifter, and every pin is 3–5 V
compliant.

## SPI settings

| Setting | Value | Source |
| --- | --- | --- |
| Clock | start at **6 MHz** | Adafruit's CircuitPython driver: *"6 MHz was about the fastest that it would stably run"* |
| Clock during init | slow, until the RA8875 PLL is set | the PLL is not running at reset; the Adafruit Arduino driver uses a slow clock for this phase |
| Mode | CPOL 0, CPHA 0 (`MODE_0`) | measured 2026-11-16 with `src/bin/ra8875_bringup.rs`: chip ID `0x75` read back in mode 0 at 1 MHz and at 5.77 MHz |
| CS | low while clocking, high between transactions | active low |

The real maximum clock with jumper wires is still the open item in `docs/device-v2.md`.

## The previous panel (SHARP 2.7")

Until the RA8875 driver lands, `device/src/main.rs` still drives the Adafruit 2.7" 400×240 SHARP
Memory Display (PID 4694). Its wiring shared SPI0 SCK/MOSI on GP18/GP19, used GP17 as an **active
high** CS and GP16 as `DISP`, and took 5 V from VBUS on `VIN`. GP16 is MISO on the RA8875, so
the two panels cannot stay wired at the same time. The SHARP SPI settings were `MODE_1`, 12 MHz,
MSB first.

## Bring-up results (2026-11-16)

`src/bin/ra8875_bringup.rs` proves every wire. Run it with
`cargo run --release --bin ra8875_bringup` and read the RTT output.

- Chip ID `0x75` in SPI mode 0, at 1 MHz and at 5.77 MHz.
- `WAIT` goes low while the drawing engine runs: 1.35 ms for a filled 120×272 rectangle.
- The backlight follows PWM1 (`P1DCR`), so `LITE` stays unconnected.
- Red, green, blue and white bars show in the correct order and fill the panel.
- `GPIOX` reads back `0x00` after a write of `0x01`, but the panel works. A read of this register
  does not return the value that was written.

### Touch

Raw 10-bit values at the four corners, touched a little inside the edge. Axes are not swapped and
not inverted: raw x grows left to right, raw y grows top to bottom.

| Corner | Raw x | Raw y |
| --- | --- | --- |
| top left | 71 | 148 |
| top right | 940 | 149 |
| bottom left | 88 | 878 |
| bottom right | 942 | 874 |

First approximation: `px = (x - 71) * 480 / (940 - 71)`, `py = (y - 148) * 272 / (874 - 148)`,
clamped to the panel. The first one or two samples of a touch are 15 to 40 counts off the settled
value, so the driver must discard the start of each touch.
