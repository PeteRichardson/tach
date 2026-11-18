//! RA8875 backend: the ADR-0004 operations as RA8875 register writes, for the 4.3" 480x272 TFT
//! on the Adafruit driver board. Wiring and the bring-up evidence: device/WIRING.md.
//!
//! Protocol, as measured with src/bin/ra8875_bringup.rs and as the Adafruit_RA8875 library does it:
//!
//! * SPI mode 0, MSB first, CS active low.
//! * Every transaction is two bytes: a prefix that says what the second byte is, then the byte.
//!   `0x80 reg` selects a register, `0x00 val` writes it, `0x40 0` reads it.
//! * The PLL is off at reset. The bus must stay slow (1 MHz) until `init` has set it.
//!
//! Double buffering: at 480x272 and 16 bpp the controller holds two layers. Every operation draws
//! on layer 2, and `present` copies the frame onto layer 1, which is always on the glass. Nothing
//! half-drawn reaches the glass.

use crate::ops::{Ink, Op, Panel};
use embedded_hal::digital::{InputPin, OutputPin};
use embedded_hal::spi::SpiBus;

/// A way to make RA8875 accesses: a prefix byte, then a data byte, with CS low across both.
pub trait Link {
    /// One access. It can return before the access is on the wire.
    fn send(&mut self, prefix: u8, byte: u8);
    /// One access, and the byte that came back during its data byte.
    fn exchange(&mut self, prefix: u8, byte: u8) -> u8;
    /// Wait until every access sent is on the wire.
    fn flush(&mut self);

    fn write_reg(&mut self, reg: u8, val: u8) {
        self.send(CMD_WRITE, reg);
        self.send(DATA_WRITE, val);
    }

    fn read_reg(&mut self, reg: u8) -> u8 {
        self.send(CMD_WRITE, reg);
        self.exchange(DATA_READ, 0)
    }
}

pub const WIDTH: u16 = 480;
pub const HEIGHT: u16 = 272;

/// The value of register 0x00 on a live RA8875.
pub const CHIP_ID: u8 = 0x75;

/// Row colours, 24-bit RGB, top to bottom: v1's CPU, GPU and MEM colours (code.py, METRICS).
const ROW_RGB: [u32; 3] = [0x40C0FF, 0x60FF60, 0xFFB040];
/// Core bar colours, by `Ink::Led` kind: super cores in the CPU row's cyan, performance cores
/// in violet. The bar's gradient goes up through yellow to red.
const CORE_RGB: [u32; 2] = [0x40C0FF, 0x9678FF];
const LED_YELLOW: u32 = 0xFFDC28;
const LED_RED: u32 = 0xFF281E;

/// `a` to `b` at `t` of 255.
fn mix(a: u32, b: u32, t: u32) -> u32 {
    (0..3).fold(0, |rgb, i| {
        let (x, y) = ((a >> (8 * i)) & 0xFF, (b >> (8 * i)) & 0xFF);
        rgb | ((x * (255 - t) + y * t) / 255) << (8 * i)
    })
}

/// The colour of a core kind. A kind past the table takes the dim colour.
fn core_rgb(kind: u8) -> u32 {
    CORE_RGB.get(kind as usize).copied().unwrap_or(DIM_RGB)
}

/// Where a core bar's gradient reaches the core's bright tone (60 %) and yellow (80 %), of 255.
const AT_BRIGHT: u32 = 153;
const AT_YELLOW: u32 = 204;

/// A lit core segment at height `f` of 255: the core's dark tone (about a third) to its bright
/// tone at 60 %, to yellow at 80 %, to red at 100 %.
fn led(kind: u8, f: u8) -> u32 {
    let (rgb, f) = (core_rgb(kind), f as u32);
    if f < AT_BRIGHT {
        mix(mix(BACKGROUND_RGB, rgb, 90), rgb, f * 255 / AT_BRIGHT)
    } else if f < AT_YELLOW {
        mix(rgb, LED_YELLOW, (f - AT_BRIGHT) * 255 / (AT_YELLOW - AT_BRIGHT))
    } else {
        mix(LED_YELLOW, LED_RED, (f - AT_YELLOW) * 255 / (255 - AT_YELLOW))
    }
}

/// The dim level: v1's colour for the memory scale.
const DIM_RGB: u32 = 0x808080;
const BACKGROUND_RGB: u32 = 0x000000;

/// 24-bit RGB to the panel's RGB565.
const fn rgb565(rgb: u32) -> u16 {
    (((rgb >> 8) & 0xF800) | ((rgb >> 5) & 0x07E0) | ((rgb >> 3) & 0x001F)) as u16
}

/// The colour of a level on this panel (docs/device-v2.md, Four drawing levels). The grid is its
/// row's colour at half strength, the fill at a quarter (v1's `dim`). A row number past the table
/// takes the dim colour.
fn colour(ink: Ink) -> u16 {
    let row = |r: u8| ROW_RGB.get(r as usize).copied().unwrap_or(DIM_RGB);
    rgb565(match ink {
        Ink::Background => BACKGROUND_RGB,
        Ink::Dim => DIM_RGB,
        Ink::Grid(r) => (row(r) >> 1) & 0x7F7F7F,
        Ink::Fill(r) => (row(r) >> 2) & 0x3F3F3F,
        Ink::Line(r) => row(r),
        Ink::Led { kind, f } => led(kind, f),
        Ink::LedOff(kind) => mix(BACKGROUND_RGB, core_rgb(kind), 30),
    })
}

const CMD_WRITE: u8 = 0x80;
const DATA_WRITE: u8 = 0x00;
const DATA_READ: u8 = 0x40;

// Registers. Names are the datasheet's.
const PWRR: u8 = 0x01;
const MRWC: u8 = 0x02;
const PCSR: u8 = 0x04;
const SYSR: u8 = 0x10;
const DPCR: u8 = 0x20;
const MWCR1: u8 = 0x41;
const CURH0: u8 = 0x46;
const BECR0: u8 = 0x50;
const BECR1: u8 = 0x51;
const LTPR0: u8 = 0x52;
/// The y-register bit that selects layer 2 in a block transfer. Every operation draws on layer 2,
/// and `present` copies the result onto layer 1, which is always on the glass. A layer swap
/// (LTPR0) shows only from the next panel refresh, up to 20 ms later: the next frame then
/// started on the layer still on the glass, and the first chart drawn blinked (seen 2026-11-17).
const DRAW_LAYER: u16 = 1 << 15;
/// Most windows that `copy_only` takes.
const MAX_COPY: usize = 8;
const HSBE0: u8 = 0x54;
const VSBE0: u8 = 0x56;
const HDBE0: u8 = 0x58;
const VDBE0: u8 = 0x5A;
const BEWR0: u8 = 0x5C;
const BEHR0: u8 = 0x5E;
const FGCR0: u8 = 0x63;
const PLLC1: u8 = 0x88;
const PLLC2: u8 = 0x89;
const P1CR: u8 = 0x8A;
const P1DCR: u8 = 0x8B;
const DCR: u8 = 0x90;
const DLHSR0: u8 = 0x91;
const GPIOX: u8 = 0xC7;

/// DCR values: start a line, an outline rectangle, a filled rectangle.
const DCR_LINE: u8 = 0x80;
const DCR_RECT: u8 = 0x90;
const DCR_FILL: u8 = 0xB0;

/// The SPI bus and a GPIO chip select, for `init`: every access waits until it is on the wire.
pub struct Bus<SPI, CS> {
    spi: SPI,
    cs: CS,
}

impl<SPI: SpiBus<u8>, CS: OutputPin> Bus<SPI, CS> {
    /// CS goes high here, so the chip stays deselected from the start.
    pub fn new(spi: SPI, mut cs: CS) -> Self {
        let _ = cs.set_high();
        Self { spi, cs }
    }

    /// Give the SPI bus and CS back, to change the bus clock after `init`.
    pub fn free(self) -> (SPI, CS) {
        (self.spi, self.cs)
    }
}

impl<SPI: SpiBus<u8>, CS: OutputPin> Link for Bus<SPI, CS> {
    fn send(&mut self, prefix: u8, byte: u8) {
        let _ = self.cs.set_low();
        let _ = self.spi.write(&[prefix, byte]);
        let _ = self.spi.flush();
        let _ = self.cs.set_high();
    }

    fn exchange(&mut self, prefix: u8, byte: u8) -> u8 {
        let mut buf = [prefix, byte];
        let _ = self.cs.set_low();
        let _ = self.spi.transfer_in_place(&mut buf);
        let _ = self.cs.set_high();
        buf[1]
    }

    fn flush(&mut self) {}
}

/// Reset the chip, check its ID, and set the PLL, the 480x272 timing and two layers. Run this
/// with the bus at 1 MHz; the bus can go faster after it returns. `Err` carries the ID that was
/// read: 0x00 points at MISO or CS, 0xff at power or a floating MISO.
pub fn init<RST: OutputPin>(
    bus: &mut impl Link,
    rst: &mut RST,
    mut wait_us: impl FnMut(u64),
) -> Result<(), u8> {
    let _ = rst.set_low();
    wait_us(100_000);
    let _ = rst.set_high();
    wait_us(100_000);

    let id = bus.read_reg(0x00);
    if id != CHIP_ID {
        return Err(id);
    }

    // PLL and pixel clock: the Adafruit_RA8875 values for 480x272.
    bus.write_reg(PLLC1, 0x0A); // PLLDIV1 + 10
    wait_us(1_000);
    bus.write_reg(PLLC2, 0x02); // DIV4
    wait_us(1_000);
    bus.write_reg(SYSR, 0x0C); // 16 bpp, 8-bit MCU interface
    bus.write_reg(PCSR, 0x82); // falling-edge fetch, system clock / 4
    wait_us(1_000);

    // Horizontal: width, then non-display 10, sync start 8, sync width 48, sync active low.
    bus.write_reg(0x14, (WIDTH / 8 - 1) as u8);
    bus.write_reg(0x15, 0x00);
    bus.write_reg(0x16, 1);
    bus.write_reg(0x17, 0);
    bus.write_reg(0x18, 5);
    // Vertical: height, then non-display 3, sync start 8, sync width 10, sync active low.
    bus.write_reg(0x19, ((HEIGHT - 1) & 0xFF) as u8);
    bus.write_reg(0x1A, ((HEIGHT - 1) >> 8) as u8);
    bus.write_reg(0x1B, 2);
    bus.write_reg(0x1C, 0);
    bus.write_reg(0x1D, 7);
    bus.write_reg(0x1E, 0);
    bus.write_reg(0x1F, 9);

    // Active window: the whole panel.
    for (reg, val) in [
        (0x30, 0),
        (0x31, 0),
        (0x32, 0),
        (0x33, 0),
        (0x34, ((WIDTH - 1) & 0xFF) as u8),
        (0x35, ((WIDTH - 1) >> 8) as u8),
        (0x36, ((HEIGHT - 1) & 0xFF) as u8),
        (0x37, ((HEIGHT - 1) >> 8) as u8),
    ] {
        bus.write_reg(reg, val);
    }

    bus.write_reg(DPCR, 0x80); // two layers
    bus.write_reg(LTPR0, 0x00); // show layer 1
    bus.write_reg(MWCR1, 0x01); // draw on layer 2

    bus.write_reg(PWRR, 0x80); // display on
    bus.write_reg(GPIOX, 0x01); // the board's panel enable
    bus.write_reg(P1CR, 0x80 | 0x0A); // PWM1 on, clock / 1024: the backlight
    bus.write_reg(P1DCR, 0xFF);
    Ok(())
}

/// The panel. Build it with a bus that `init` has already set up.
pub struct Ra8875<L, W> {
    bus: L,
    /// The WAIT pin: low while the chip is busy and cannot take the next access.
    wait: W,
    /// Last value written to each register, or `None`. Coordinates and colour repeat a lot
    /// between operations, and an unchanged register costs no bus time.
    regs: [Option<u8>; 256],
    /// The windows that `present` copies to the glass, as (left, top, width, height). Empty means
    /// the whole panel.
    copy: [(u16, u16, u16, u16); MAX_COPY],
    copy_n: usize,
    /// Bus bytes in the last frame, from one `present` to the next.
    pub last_frame_bytes: usize,
    bytes: usize,
    /// Operations in the last frame.
    pub last_frame_ops: usize,
    ops: usize,
}

impl<L: Link, W: InputPin> Ra8875<L, W> {
    pub fn new(bus: L, wait: W) -> Self {
        Self {
            bus,
            wait,
            regs: [None; 256],
            copy: [(0, 0, 0, 0); MAX_COPY],
            copy_n: 0,
            last_frame_bytes: 0,
            bytes: 0,
            last_frame_ops: 0,
            ops: 0,
        }
    }

    /// From now on, `present` copies only these windows: the parts of the panel that change from
    /// frame to frame. At most `MAX_COPY`; the rest are dropped.
    pub fn copy_only(&mut self, windows: &[(u16, u16, u16, u16)]) {
        self.copy_n = windows.len().min(MAX_COPY);
        self.copy[..self.copy_n].copy_from_slice(&windows[..self.copy_n]);
    }

    /// Wait until the chip can take the next access. The busy bits cannot be read at the drawing
    /// clock (see `SPI_HZ` in main.rs), so this polls the WAIT pin. Measured 2026-11-16: once WAIT
    /// is high, STSR showed the drawing engine (bits 7, 6) and the block transfer engine (bit 6)
    /// idle, also at the end of 30,000 glyphs.
    fn ready(&mut self) {
        // The chip can only be busy with what it has received.
        self.bus.flush();
        while self.wait.is_low().unwrap_or(false) {}
    }

    fn write_reg(&mut self, reg: u8, val: u8) {
        self.bus.write_reg(reg, val);
        self.bytes += 4;
    }

    fn send(&mut self, prefix: u8, byte: u8) {
        self.bus.send(prefix, byte);
        self.bytes += 2;
    }

    /// Write a register only if it does not hold `val` already.
    fn set(&mut self, reg: u8, val: u8) {
        if self.regs[reg as usize] != Some(val) {
            self.write_reg(reg, val);
            self.regs[reg as usize] = Some(val);
        }
    }

    /// A 10-bit coordinate pair at `reg` (low byte) and `reg + 1` (high byte).
    fn set_xy(&mut self, reg: u8, v: u16) {
        self.set(reg, (v & 0xFF) as u8);
        self.set(reg + 1, (v >> 8) as u8);
    }

    fn set_color(&mut self, ink: Ink) {
        let c = colour(ink);
        self.set(FGCR0, ((c & 0xF800) >> 11) as u8);
        self.set(FGCR0 + 1, ((c & 0x07E0) >> 5) as u8);
        self.set(FGCR0 + 2, (c & 0x001F) as u8);
    }

    /// Run the drawing engine on (x0, y0)-(x1, y1) and wait for it. A shape fully outside the
    /// panel is skipped; other coordinates outside it are clamped onto its edge, since the
    /// registers are unsigned.
    fn shape(&mut self, dcr: u8, x0: i32, y0: i32, x1: i32, y1: i32, ink: Ink) {
        let (w, h) = (WIDTH as i32, HEIGHT as i32);
        if x0.max(x1) < 0 || x0.min(x1) >= w || y0.max(y1) < 0 || y0.min(y1) >= h {
            return;
        }
        let (x0, x1) = (x0.clamp(0, w - 1) as u16, x1.clamp(0, w - 1) as u16);
        let (y0, y1) = (y0.clamp(0, h - 1) as u16, y1.clamp(0, h - 1) as u16);
        if (x0, y0) == (x1, y1) {
            // The engine hangs on a shape that starts and ends on one point: it stays busy and
            // takes no more drawing. Write the pixel straight to memory instead. Compare after
            // the clamp: a short line off the edge clamps to one point too.
            return self.pixel(x0, y0, ink);
        }
        self.set_xy(DLHSR0, x0);
        self.set_xy(DLHSR0 + 2, y0);
        self.set_xy(DLHSR0 + 4, x1);
        self.set_xy(DLHSR0 + 6, y1);
        self.set_color(ink);
        // DCR is a command, not a value: always write it.
        self.write_reg(DCR, dcr);
        self.ready();
    }

    /// One pixel, through the memory write port. The cursor moves on after the write, so its
    /// registers are written every time and not cached.
    fn pixel(&mut self, x: u16, y: u16, ink: Ink) {
        for (reg, val) in [
            (CURH0, (x & 0xFF) as u8),
            (CURH0 + 1, (x >> 8) as u8),
            (CURH0 + 2, (y & 0xFF) as u8),
            (CURH0 + 3, (y >> 8) as u8),
        ] {
            self.write_reg(reg, val);
            self.regs[reg as usize] = None;
        }
        let c = colour(ink);
        self.send(CMD_WRITE, MRWC);
        self.send(DATA_WRITE, (c >> 8) as u8);
        self.send(DATA_WRITE, (c & 0xFF) as u8);
        self.ready();
    }

    fn rect(&mut self, x: i32, y: i32, w: u16, h: u16, fill: bool, ink: Ink) {
        if w == 0 || h == 0 {
            return;
        }
        let dcr = if fill { DCR_FILL } else { DCR_RECT };
        self.shape(dcr, x, y, x + w as i32 - 1, y + h as i32 - 1, ink);
    }

    /// Move a window's pixels left by `by` with the block transfer engine, then clear the strip
    /// that it vacates on the right.
    fn shift_band(&mut self, left: u16, top: u16, width: u16, height: u16, by: usize) {
        let by = (by as u16).min(width);
        if by < width {
            let layer = DRAW_LAYER;
            self.set_xy(HSBE0, left + by);
            self.set_xy(VSBE0, top | layer);
            self.set_xy(HDBE0, left);
            self.set_xy(VDBE0, top | layer);
            self.set_xy(BEWR0, width - by);
            self.set_xy(BEHR0, height);
            self.set(BECR1, 0xC2); // ROP "source", move in the positive direction
            self.write_reg(BECR0, 0x80);
            // Not measured: the charts do not shift at present. A glyph's end was (see ready).
            self.ready();
        }
        let x = (left + width - by) as i32;
        self.rect(x, top as i32, by, height, true, Ink::Background);
    }

    /// A glyph, as a 1 bpp bitmap that the block transfer engine expands to the foreground colour.
    /// Unlit pixels are transparent, as the operation requires. The bitmap goes over the bus as it
    /// is in the atlas: rows padded to whole bytes, first pixel in the high bit.
    fn glyph(&mut self, x: i32, y: i32, glyph: &crate::text::Glyph, ink: Ink) {
        // The engine takes `w` bits for each row and does not skip the atlas's padding to whole
        // bytes, so a glyph not 8 px wide comes out sheared. Give it the padded width: the padding
        // bits are 0, which is transparent here.
        let (w, h) = (glyph.w.div_ceil(8) as u16 * 8, glyph.h as u16);
        if w == 0 || h == 0 {
            return;
        }
        if x < 0 || y < 0 || x + w as i32 > WIDTH as i32 || y + h as i32 > HEIGHT as i32 {
            // The engine does not clip. A glyph across the edge goes the slow way.
            return self.glyph_runs(x, y, glyph, ink);
        }
        let layer = DRAW_LAYER;
        self.set_xy(HDBE0, x as u16);
        self.set_xy(VDBE0, y as u16 | layer);
        self.set_xy(BEWR0, w);
        self.set_xy(BEHR0, h);
        self.set(BECR1, 0x79); // colour expansion with transparency, first pixel in bit 7
        self.set_color(ink);
        self.write_reg(BECR0, 0x80);
        self.send(CMD_WRITE, MRWC);
        // One byte at a time, each after the engine is ready for it, as RAiO's sample code does. In
        // one burst, a 36-byte name or an 81-byte digit lost bytes; a 12-byte small glyph did not.
        for &byte in glyph.bits {
            self.send(DATA_WRITE, byte);
            self.ready();
        }
        self.ready();
    }

    /// A glyph, as one horizontal line per run of lit pixels.
    fn glyph_runs(&mut self, x: i32, y: i32, glyph: &crate::text::Glyph, ink: Ink) {
        for gy in 0..glyph.h {
            let mut gx = 0;
            while gx < glyph.w {
                if !glyph.lit(gx, gy) {
                    gx += 1;
                    continue;
                }
                let start = gx;
                while gx < glyph.w && glyph.lit(gx, gy) {
                    gx += 1;
                }
                let py = y + gy as i32;
                self.shape(DCR_LINE, x + start as i32, py, x + gx as i32 - 1, py, ink);
            }
        }
    }
}

impl<L: Link, W: InputPin> Panel for Ra8875<L, W> {
    fn size(&self) -> (u16, u16) {
        (WIDTH, HEIGHT)
    }

    fn apply(&mut self, op: Op) {
        self.ops += 1;
        match op {
            Op::Clear { ink } => self.rect(0, 0, WIDTH, HEIGHT, true, ink),
            Op::Pix { x, y, ink } => self.shape(DCR_LINE, x, y, x, y, ink),
            Op::Line { x0, y0, x1, y1, ink } => self.shape(DCR_LINE, x0, y0, x1, y1, ink),
            Op::Rect { x, y, w, h, fill: true, ink, .. } => self.rect(x, y, w, h, true, ink),
            Op::Rect { x, y, w, h, fill: false, t, ink } => {
                // Nested 1 px outlines, growing outwards, as Framebuf::outline does.
                for i in 0..t {
                    self.rect(x - i as i32, y - i as i32, w + 2 * i, h + 2 * i, false, ink);
                }
            }
            Op::ShiftBand { left, top, width, height, by } => {
                self.shift_band(left, top, width, height, by)
            }
            Op::Glyph { x, y, glyph, ink } => self.glyph(x, y, glyph, ink),
            Op::Brightness { percent } => {
                self.set(P1DCR, (percent.min(100) as u16 * 255 / 100) as u8)
            }
        }
    }

    /// Copy the frame from layer 2 onto layer 1, the one on the glass: the whole panel, or only
    /// the windows that `copy_only` set.
    fn present(&mut self) {
        let (windows, n) = match self.copy_n {
            0 => ([(0, 0, WIDTH, HEIGHT); MAX_COPY], 1),
            n => (self.copy, n),
        };
        for &(x, y, w, h) in &windows[..n] {
            self.set_xy(HSBE0, x);
            self.set_xy(VSBE0, y | DRAW_LAYER);
            self.set_xy(HDBE0, x);
            self.set_xy(VDBE0, y);
            self.set_xy(BEWR0, w);
            self.set_xy(BEHR0, h);
            self.set(BECR1, 0xC2); // ROP "source", move in the positive direction
            self.write_reg(BECR0, 0x80);
            self.ready();
        }
        self.last_frame_bytes = core::mem::take(&mut self.bytes);
        self.last_frame_ops = core::mem::take(&mut self.ops);
    }
}
