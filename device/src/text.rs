//! The number slot: the glyph atlas, the number formats, and where the label and the number go.
//!
//! The atlas is baked at build time from the BDF files in fonts/ (device/bake_fonts.rs, ADR-0001),
//! so there is no font file on the device. The fonts come from Monaspace Neon SemiBold: see
//! fonts/make-fonts.py and fonts/OFL.txt.

use core::fmt::Write as _;

use crate::layout::Row;
use crate::ops::Op;

/// One character of the atlas: 1 bpp, rows top to bottom, each row padded to whole bytes, the
/// first pixel in the high bit (the BDF bitmap as is).
#[derive(Debug, PartialEq, Eq)]
pub struct Glyph {
    pub ch: u8,
    /// Pen travel to the next character.
    pub adv: u8,
    /// Left edge of the ink from the pen position.
    pub x: i8,
    /// Top edge of the ink from the baseline, y down, so negative above the baseline.
    pub top: i8,
    pub w: u8,
    pub h: u8,
    pub bits: &'static [u8],
}

impl Glyph {
    /// True if pixel (x, y) of the ink box is lit.
    pub fn lit(&self, x: u8, y: u8) -> bool {
        let stride = (self.w as usize).div_ceil(8);
        self.bits[y as usize * stride + x as usize / 8] & (0x80 >> (x % 8)) != 0
    }
}

pub struct Font {
    pub glyphs: &'static [Glyph],
}

impl Font {
    fn get(&self, ch: u8) -> Option<&'static Glyph> {
        self.glyphs.iter().find(|g| g.ch == ch)
    }

    /// Pen travel of `text`. A character not in the atlas counts as nothing.
    pub fn width(&self, text: &[u8]) -> i32 {
        text.iter().filter_map(|&c| self.get(c)).map(|g| g.adv as i32).sum()
    }

    /// Draw `text` with the pen starting at `x` on `baseline`. Only the ink is drawn; the caller
    /// clears the background first.
    pub fn draw(&self, text: &[u8], x: i32, baseline: i32, on: bool, mut op: impl FnMut(Op)) {
        let mut pen = x;
        for g in text.iter().filter_map(|&c| self.get(c)) {
            op(Op::Glyph { x: pen + g.x as i32, y: baseline + g.top as i32, glyph: g, on });
            pen += g.adv as i32;
        }
    }
}

include!(concat!(env!("OUT_DIR"), "/fonts.rs"));

/// The text of one number, at most "100%" or "97.4G" in use. No allocator, so a fixed buffer.
pub struct Number {
    buf: [u8; 8],
    len: usize,
}

impl Number {
    /// What a stale or unknown row shows: a dead source must not look live (issue #1).
    pub fn dashes() -> Self {
        Self { buf: *b"--\0\0\0\0\0\0", len: 2 }
    }

    /// A percent, whole: "7%", "100%" (the v1 format).
    pub fn percent(v: f32) -> Self {
        Self::format(format_args!("{:.0}%", v.clamp(0.0, 999.0)))
    }

    /// Gigabytes: one decimal below 100, whole from there, so the text stays at five characters
    /// ("97.4G", "128G", the v1 format).
    pub fn gb(v: f32) -> Self {
        let v = v.clamp(0.0, 9999.0);
        if v < 99.95 {
            Self::format(format_args!("{:.1}G", v))
        } else {
            Self::format(format_args!("{:.0}G", v))
        }
    }

    fn format(args: core::fmt::Arguments) -> Self {
        let mut n = Self { buf: [0; 8], len: 0 };
        if n.write_fmt(args).is_err() {
            return Self::dashes();
        }
        n
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

impl core::fmt::Write for Number {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let end = self.len + s.len();
        if end > self.buf.len() {
            return Err(core::fmt::Error);
        }
        self.buf[self.len..end].copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

/// Clear the number slot of `row`, then draw its label at the top left and its number on the
/// chart's bottom line, right-aligned so that "%" and "G" line up in one column on every row.
pub fn draw_slot(row: &Row, label: &[u8], number: &Number, mut op: impl FnMut(Op)) {
    let (x, y, w, h) = row.slot();
    op(Op::Rect { x: x as i32, y: y as i32, w, h, fill: true, t: 0, on: false });
    LABELS.draw(label, x as i32, row.label_baseline() as i32, true, &mut op);
    let text = number.as_bytes();
    let right = row.number_right() as i32;
    DIGITS.draw(text, right - DIGITS.width(text), row.number_baseline() as i32, true, &mut op);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout;

    fn ink(label: &[u8], number: &Number) -> Vec<(i32, i32)> {
        let row = layout::rows(400, 240)[0];
        let mut lit = Vec::new();
        draw_slot(&row, label, number, |op| {
            if let Op::Glyph { x, y, glyph, .. } = op {
                for gy in 0..glyph.h {
                    for gx in 0..glyph.w {
                        if glyph.lit(gx, gy) {
                            lit.push((x + gx as i32, y + gy as i32));
                        }
                    }
                }
            }
        });
        lit
    }

    #[test]
    fn the_atlas_has_every_character_the_slot_uses() {
        for c in b"0123456789.%G-" {
            assert!(DIGITS.get(*c).is_some(), "digit atlas lacks {}", *c as char);
        }
        for c in b"CPUGME" {
            assert!(LABELS.get(*c).is_some(), "label atlas lacks {}", *c as char);
        }
    }

    #[test]
    fn every_digit_sits_on_one_baseline() {
        // A digit 1 px off makes the number jump when that digit comes or goes.
        for c in b"0123456789" {
            let g = DIGITS.get(*c).unwrap();
            assert_eq!(g.top as i32 + g.h as i32, 0, "{} does not end on the baseline", *c as char);
        }
    }

    #[test]
    fn numbers_use_the_v1_formats() {
        let t = |n: Number| String::from_utf8(n.as_bytes().to_vec()).unwrap();
        assert_eq!(t(Number::percent(7.4)), "7%");
        assert_eq!(t(Number::percent(100.0)), "100%");
        assert_eq!(t(Number::percent(-3.0)), "0%");
        assert_eq!(t(Number::gb(97.43)), "97.4G");
        assert_eq!(t(Number::gb(99.96)), "100G");
        assert_eq!(t(Number::gb(128.0)), "128G");
        assert_eq!(t(Number::dashes()), "--");
    }

    #[test]
    fn the_widest_numbers_stay_inside_the_slot() {
        let row = layout::rows(400, 240)[0];
        let (x, y, w, h) = row.slot();
        for n in [Number::gb(97.4), Number::gb(9999.0), Number::percent(100.0)] {
            for (px, py) in ink(b"MEM", &n) {
                assert!(px >= x as i32 && px < (x + w) as i32, "x {px} for {:?}", n.as_bytes());
                assert!(py >= y as i32 && py < (y + h) as i32, "y {py} for {:?}", n.as_bytes());
            }
        }
    }

    #[test]
    fn the_units_line_up_in_one_column() {
        let right = |n: Number| ink(b"CPU", &n).iter().map(|p| p.0).max().unwrap();
        assert_eq!(right(Number::percent(7.0)), right(Number::percent(100.0)));
        // "%" and "G" have different ink, but the same pen end; both end within 2 px.
        assert!((right(Number::percent(7.0)) - right(Number::gb(97.4))).abs() <= 2);
    }

    #[test]
    fn the_label_and_the_number_do_not_touch() {
        let lit = ink(b"MEM", &Number::gb(88.8));
        let row = layout::rows(400, 240)[0];
        let label_bottom = lit.iter().filter(|p| p.1 <= row.label_baseline() as i32).map(|p| p.1).max();
        let number_top = lit.iter().filter(|p| p.1 > row.label_baseline() as i32).map(|p| p.1).min();
        assert!(number_top.unwrap() - label_bottom.unwrap() > 4);
    }
}
