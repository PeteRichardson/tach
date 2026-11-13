//! One-bit frame buffer with per-row dirty tracking.
//!
//! Bits are stored in wire order: the bit for pixel `x` is bit `7 - (x % 8)` of byte `x / 8`, so
//! a row can be sent to the panel without any bit shuffling. The SHARP panel clocks the first
//! pixel of a byte out first, which is why this is not the "obvious" little-endian layout.
//! See docs/device-v2.md, Mac-side sources and device/WIRING.md.

use crate::ops::Madrs;
use crate::text::Glyph;

/// Bytes in one panel row at 400 px wide.
pub const ROW_BYTES: usize = 50;
/// Panel width of the scaffold, in pixels.
pub const WIDTH: usize = 400;
/// Panel height of the scaffold, in pixels.
pub const HEIGHT: usize = 240;

/// A monochrome image in MCU RAM, plus the rows that changed since the last flush.
pub struct Framebuf {
    rows: [[u8; ROW_BYTES]; HEIGHT],
    dirty: [bool; HEIGHT],
    /// Background level: true lights pixels, so the panel is "upside down" if this is true.
    background_on: bool,
}

impl Framebuf {
    /// A blank buffer with the background off (the panel's natural unlit grey).
    pub fn new() -> Self {
        Self {
            rows: [[0u8; ROW_BYTES]; HEIGHT],
            dirty: [true; HEIGHT],
            background_on: false,
        }
    }

    /// Width in pixels.
    pub fn width(&self) -> usize {
        WIDTH
    }

    /// Height in pixels.
    pub fn height(&self) -> usize {
        HEIGHT
    }

    /// Set which level counts as the background, for the mono level map.
    pub fn set_background_on(&mut self, on: bool) {
        self.background_on = on;
    }

    /// Light or darken one pixel. Out-of-range coordinates are ignored, not fatal: a layout bug
    /// must not panic a device that is bolted to a monitor.
    #[inline]
    pub fn set(&mut self, x: i32, y: i32, on: bool) {
        if x < 0 || y < 0 || x >= WIDTH as i32 || y >= HEIGHT as i32 {
            return;
        }
        let (x, y) = (x as usize, y as usize);
        let bit = 7 - (x % 8);
        let want = on != self.background_on;
        if want {
            self.rows[y][x / 8] |= 1 << bit;
        } else {
            self.rows[y][x / 8] &= !(1 << bit);
        }
        self.dirty[y] = true;
    }

    /// Light a pixel if `on` matches the background level, inverted as needed.
    #[inline]
    pub fn set_level(&mut self, x: i32, y: i32, foreground: bool) {
        self.set(x, y, foreground);
    }

    /// Fill a rectangle. Coordinates are inclusive of the top-left and exclusive of the
    /// bottom-right, like every other graphics API.
    ///
    /// Whole bytes at a time, through a mask of the span: the chart clears each plot area on every
    /// frame, and a per-pixel loop cost 8.4 ms per frame for that alone.
    pub fn fill_rect(&mut self, x: i32, y: i32, w: u16, h: u16, on: bool) {
        let x0 = x.clamp(0, WIDTH as i32) as usize;
        let x1 = (x + w as i32).clamp(0, WIDTH as i32) as usize;
        let y0 = y.clamp(0, HEIGHT as i32) as usize;
        let y1 = (y + h as i32).clamp(0, HEIGHT as i32) as usize;
        if x0 >= x1 {
            return;
        }
        let mask = span_mask(x0, x1);
        let fill = if on != self.background_on { 0xff } else { 0x00 };
        let (b0, b1) = (x0 / 8, (x1 - 1) / 8 + 1);
        for yy in y0..y1 {
            let row = &mut self.rows[yy];
            for i in b0..b1 {
                row[i] = (row[i] & !mask[i]) | (fill & mask[i]);
            }
            self.dirty[yy] = true;
        }
    }

    /// Draw the lit pixels of a glyph with its ink box at (x, y). Unlit pixels are left alone.
    pub fn glyph(&mut self, x: i32, y: i32, g: &Glyph, on: bool) {
        for gy in 0..g.h {
            for gx in 0..g.w {
                if g.lit(gx, gy) {
                    self.set(x + gx as i32, y + gy as i32, on);
                }
            }
        }
    }

    /// Rectangle outline, `t` pixels thick on each side.
    pub fn outline(&mut self, x: i32, y: i32, w: u16, h: u16, t: u16, on: bool) {
        let (ww, hh) = (w as i32, h as i32);
        for i in 0..t as i32 {
            self.fill_rect(x, y - i, w, 1, on);
            self.fill_rect(x, y + hh - 1 + i, w, 1, on);
            self.fill_rect(x - i, y, 1, hh as u16, on);
            self.fill_rect(x + ww - 1 + i, y, 1, hh as u16, on);
        }
    }

    /// Horizontal line.
    pub fn hline(&mut self, x: i32, y: i32, len: u16, on: bool) {
        self.fill_rect(x, y, len, 1, on);
    }

    /// Vertical line.
    pub fn vline(&mut self, x: i32, y: i32, len: u16, on: bool) {
        self.fill_rect(x, y, 1, len, on);
    }

    /// Bresenham line, integer only. Used for the axis rules and the self-test marks, not for the
    /// chart curve — the RA8875 draws curves in hardware.
    pub fn line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, on: bool) {
        let (mut x, mut y) = (x0, y0);
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx: i32 = if x0 < x1 { 1 } else { -1 };
        let sy: i32 = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        loop {
            self.set(x, y, on);
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    /// Move the pixels of a window `by` pixels to the left. The leftmost `by` columns of the window
    /// are dropped and the rightmost `by` columns become background. Pixels outside the window do
    /// not move, so a chart frame drawn around the window stays where it is. This is what the
    /// RA8875 hardware scroll window does in one command; the shim has to do it in software.
    ///
    /// Not `[u8]::rotate_left`: that turns the row by whole bytes (8 pixels) and wraps the dropped
    /// byte round to the right edge.
    pub fn shift_band_left(&mut self, left: u16, top: u16, width: u16, height: u16, by: usize) {
        let x0 = (left as usize).min(WIDTH);
        let x1 = (left as usize + width as usize).min(WIDTH);
        let y1 = (top as usize + height as usize).min(HEIGHT);
        // Whole bytes at a time: shift each byte left by `by` bits, carrying in bits from the
        // bytes to its right, then keep the result only inside the window. A per-pixel loop gave
        // the same pixels but cost 7.6 ms per frame instead of well under 1 ms.
        let window = span_mask(x0, x1);
        let vacated = span_mask(x1.saturating_sub(by).max(x0), x1);
        let fill = if self.background_on { 0xff } else { 0x00 };
        let (bytes, bits) = (by / 8, (by % 8) as u32);
        for y in (top as usize).min(HEIGHT)..y1 {
            let src = self.rows[y];
            let at = |i: usize| if i < ROW_BYTES { src[i] } else { fill };
            let row = &mut self.rows[y];
            for i in 0..ROW_BYTES {
                if window[i] == 0 {
                    continue;
                }
                let hi = at(i + bytes);
                let shifted = if bits == 0 {
                    hi
                } else {
                    (hi << bits) | (at(i + bytes + 1) >> (8 - bits))
                };
                let moved = window[i] & !vacated[i];
                row[i] = (src[i] & !window[i]) | (shifted & moved) | (fill & vacated[i]);
            }
            self.dirty[y] = true;
        }
    }

    /// Paint the whole panel.
    pub fn fill(&mut self, on: bool) {
        let byte = if on != self.background_on { 0xff } else { 0x00 };
        for row in self.rows.iter_mut() {
            *row = [byte; ROW_BYTES];
        }
        self.dirty = [true; HEIGHT];
    }

    /// Take the dirty flags out of the buffer, clearing them. The driver collects these first so it
    /// can read row bytes while it still holds the buffer borrowed immutably.
    pub fn take_dirty_flags(&mut self) -> [bool; HEIGHT] {
        let mut out = [false; HEIGHT];
        core::mem::swap(&mut out, &mut self.dirty);
        out
    }

    /// Mark every row dirty, for a forced full refresh.
    pub fn mark_all_dirty(&mut self) {
        self.dirty = [true; HEIGHT];
    }

    /// Raw row bytes, for the panel driver.
    pub fn row(&self, y: usize) -> &[u8; ROW_BYTES] {
        &self.rows[y]
    }

    /// Fill the whole panel with `on`, ignoring the background inversion. Used by the self-test so
    /// that "all pixels lit" means exactly that.
    pub fn fill_raw(&mut self, on: bool) {
        let byte = if on { 0xff } else { 0x00 };
        for row in self.rows.iter_mut() {
            *row = [byte; ROW_BYTES];
        }
        self.dirty = [true; HEIGHT];
    }

    /// Translate a [`Madrs`] flip into row and column mapping for the self-test.
    pub fn flip(&self, m: Madrs, x: usize, y: usize) -> (usize, usize) {
        match m {
            Madrs::Normal => (x, y),
            Madrs::MirrorX => (WIDTH - 1 - x, y),
            Madrs::MirrorY => (x, HEIGHT - 1 - y),
            Madrs::MirrorXY => (WIDTH - 1 - x, HEIGHT - 1 - y),
        }
    }
}

impl Default for Framebuf {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-byte masks of the bits for pixels in `[a, b)` of a row, in wire order.
fn span_mask(a: usize, b: usize) -> [u8; ROW_BYTES] {
    let mut m = [0u8; ROW_BYTES];
    for (i, byte) in m.iter_mut().enumerate() {
        let lo = a.max(i * 8);
        let hi = b.min(i * 8 + 8);
        if lo < hi {
            *byte = ((0xffu16 << (8 - (hi - lo))) as u8) >> (lo - i * 8);
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The byte-wise fill must light exactly the pixels the per-pixel loop did.
    #[test]
    fn fill_rect_matches_the_pixel_loop() {
        for &(x, y, w, h, bg) in &[
            (0, 0, 400, 240, false),
            (3, 5, 1, 1, false),
            (7, 2, 2, 3, true),
            (119, 4, 276, 60, false),
            (-5, -5, 20, 10, true),
            (390, 230, 50, 50, false),
            (8, 0, 8, 1, false),
        ] {
            for on in [true, false] {
                let mut fast = Framebuf::new();
                let mut slow = Framebuf::new();
                for fb in [&mut fast, &mut slow] {
                    fb.set_background_on(bg);
                    fb.fill_raw(!on);
                }
                fast.fill_rect(x, y, w, h, on);
                for yy in y..y + h as i32 {
                    for xx in x..x + w as i32 {
                        slow.set(xx, yy, on);
                    }
                }
                for r in 0..HEIGHT {
                    assert_eq!(fast.row(r), slow.row(r), "rect {:?} on {} row {}", (x, y, w, h, bg), on, r);
                }
            }
        }
    }
}
