//! SHARP Memory Display driver, 400x240, 1 bpp, ported to embedded-hal 1.0.
//!
//! Why this file exists instead of `sharp-memory-display`: that crate (0.3.0, the newest release)
//! is built against embedded-hal 0.2 and embedded-graphics 0.7, while rp235x-hal 0.4 is on
//! embedded-hal 1.0. Rather than carry two generations of both crates, the protocol is ported here
//! from the reference source. The protocol itself is unchanged and is documented inline.
//!
//! Protocol, per the SHARP Memory LCD programming note and the reference driver:
//!
//! * SPI mode 1 (CPOL 0, CPHA 1), MSB first.
//! * **CS is active high** — asserted by driving it high, the opposite of normal SPI.
//! * One frame: assert CS, one command byte, then one record per changed row, then one dummy byte,
//!   then deassert CS.
//! * Command byte: bit7 = write-line (0x80), bit5 = clear memory (0x20), bit6 = VCOM (0x40).
//! * Row record: line number, 50 data bytes, one dummy byte.
//! * **The line number is bit-reversed**: the panel takes the address LSB first on the wire, while
//!   pixel data goes MSB first (pixel 0 of the byte first). Our frame buffer already stores pixels
//!   in wire order, so only the address needs reversing.
//! * VCOM must toggle on every frame. Memory LCDs are driven with an AC signal; a constant level
//!   across many frames shortens panel life.

use crate::framebuf::{Framebuf, ROW_BYTES};
use crate::ops::{Op, Panel};
use embedded_hal::digital::OutputPin;
use embedded_hal::spi::SpiBus;

/// Write one or more lines: command bit 7.
const CMD_WRITE_LINE: u8 = 0x80;
/// Clear the whole panel memory: command bit 5.
const CMD_CLEAR_MEMORY: u8 = 0x20;
/// VCOM polarity: bit 6.
const CMD_VCOM: u8 = 0x40;
/// Recommended filler byte.
const DUMMY: u8 = 0x00;

/// The panel, its two control lines and the frame buffer the shim draws into.
pub struct Sharp<SPI, CS, DISP> {
    spi: SPI,
    cs: CS,
    disp: DISP,
    fb: Framebuf,
    vcom_high: bool,
    /// Bytes moved by the last present(), for the frame budget measurement.
    pub last_flush_bytes: usize,
    /// Rows sent by the last present().
    pub last_flush_rows: usize,
    /// Panel orientation requested by the layout. The shim applies it in software at draw time by
    /// the caller; stored here so a target-panel port has somewhere to put it.
    pub madrs: crate::ops::Madrs,
}

impl<SPI, CS, DISP> Sharp<SPI, CS, DISP>
where
    SPI: SpiBus<u8>,
    CS: OutputPin,
    DISP: OutputPin,
{
    /// Wrap the SPI bus, the active-high chip select and the display-enable pin.
    pub fn new(spi: SPI, mut cs: CS, mut disp: DISP) -> Self {
        // Both idle: CS deasserted is low on this panel, and DISP low means the panel is off.
        let _ = cs.set_low();
        let _ = disp.set_low();
        Self {
            spi,
            cs,
            disp,
            fb: Framebuf::new(),
            vcom_high: true,
            last_flush_bytes: 0,
            last_flush_rows: 0,
            madrs: crate::ops::Madrs::Normal,
        }
    }

    /// Power the panel's display driver up. Wait about 20 ms after this before the first flush,
    /// per the panel note.
    pub fn enable(&mut self) {
        let _ = self.disp.set_high();
    }

    /// Power the panel down. The image stays on the glass — that is what memory-in-pixel means.
    pub fn disable(&mut self) {
        let _ = self.disp.set_low();
    }

    /// The frame buffer, for the shim and the self-test.
    pub fn framebuf(&mut self) -> &mut Framebuf {
        &mut self.fb
    }

    /// Send a command with no row data, with CS asserted around it.
    fn command(&mut self, byte: u8) {
        let _ = self.cs.set_high();
        let _ = self.spi.write(&[byte]);
        let _ = self.spi.flush();
        let _ = self.cs.set_low();
    }

    /// Ask the panel to clear its own memory. Cheap: one command, no pixel traffic.
    pub fn clear_panel(&mut self) {
        self.vcom_high = !self.vcom_high;
        self.command(CMD_CLEAR_MEMORY | if self.vcom_high { CMD_VCOM } else { 0 });
        let _ = self.cs.set_high();
        let _ = self.spi.write(&[DUMMY]);
        let _ = self.spi.flush();
        let _ = self.cs.set_low();
    }

    /// Toggle VCOM without writing pixels. Do this at least once a second while the panel is up.
    pub fn vcom_toggle(&mut self) {
        self.vcom_high = !self.vcom_high;
        self.command(if self.vcom_high { CMD_VCOM } else { 0 });
        let _ = self.cs.set_high();
        let _ = self.spi.write(&[DUMMY]);
        let _ = self.spi.flush();
        let _ = self.cs.set_low();
    }

    /// Push the changed rows to the glass, then clear the dirty marks.
    ///
    /// Only rows that changed go out, which is the whole reason the shim is not as slow as a naive
    /// full-frame push: a chart tick dirties the row band it draws in, not the panel.
    pub fn flush(&mut self) {
        let mut bytes = 1usize; // command byte
        let mut rows = 0usize;

        let _ = self.cs.set_high();
        let _ = self.spi.write(&[CMD_WRITE_LINE | if self.vcom_high { CMD_VCOM } else { 0 }]);

        let flags = self.fb.take_dirty_flags();
        for y in 0..crate::framebuf::HEIGHT {
            if !flags[y] {
                continue;
            }
            let record = {
                let data = self.fb.row(y);
                let mut buf = [0u8; 1 + ROW_BYTES + 1];
                buf[0] = (y as u8 + 1).reverse_bits();
                buf[1..=ROW_BYTES].copy_from_slice(data);
                buf[1 + ROW_BYTES] = DUMMY;
                buf
            };
            let _ = self.spi.write(&record);
            bytes += record.len();
            rows += 1;
        }

        // 16-bit frame trailer.
        let _ = self.spi.write(&[DUMMY]);
        bytes += 1;
        let _ = self.spi.flush();
        let _ = self.cs.set_low();

        self.vcom_high = !self.vcom_high;
        self.last_flush_bytes = bytes;
        self.last_flush_rows = rows;
    }
}

impl<SPI, CS, DISP> Panel for Sharp<SPI, CS, DISP>
where
    SPI: SpiBus<u8>,
    CS: OutputPin,
    DISP: OutputPin,
{
    fn size(&self) -> (u16, u16) {
        (self.fb.width() as u16, self.fb.height() as u16)
    }

    fn apply(&mut self, op: Op) {
        match op {
            Op::Clear { on } => self.fb.fill(on),
            Op::Pix { x, y, on } => self.fb.set(x, y, on),
            Op::Line { x0, y0, x1, y1, on } => self.fb.line(x0, y0, x1, y1, on),
            Op::Rect { x, y, w, h, fill, t, on } => {
                if fill {
                    self.fb.fill_rect(x, y, w, h, on);
                } else {
                    self.fb.outline(x, y, w, h, t, on);
                }
            }
            Op::ShiftBand { left, top, width, height, by } => {
                self.fb.shift_band_left(left, top, width, height, by)
            }
            Op::Glyph { x, y, glyph, on } => self.fb.glyph(x, y, glyph, on),
            // No backlight on this panel. Kept in the trait so the caller does not need to know.
            Op::Brightness { .. } => {}
        }
    }

    fn present(&mut self) {
        self.flush();
    }
}
