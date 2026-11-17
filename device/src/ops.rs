//! The drawing operations the firmware issues, and the contract every panel backend must meet.
//!
//! The firmware never builds a whole frame and ships it. It issues operations; each backend does
//! what its controller does best. See ADR-0004.

use crate::text::Glyph;

/// What a pixel shows: a drawing level, not a colour. Each backend maps it through its own table
/// (docs/device-v2.md, Four drawing levels). The number is the row, 0 to `layout::ROWS - 1`, so a
/// row's curve, name and number share its colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ink {
    Background,
    /// Marks that are not data: the panel border, the memory limits, a stale number.
    Dim,
    /// A row's chart frame and grid ticks, in a dark tone of its colour.
    Grid(u8),
    /// The area under a row's curve, darker than the grid, as v1 shades it.
    Fill(u8),
    /// A row's curve, name and number, in its colour.
    Line(u8),
}

impl Ink {
    /// The level on a 1 bpp panel, which has no grey and no colour: everything but the background
    /// is ink.
    #[allow(dead_code, reason = "used by the SHARP backend and the host tests")]
    pub fn on(self) -> bool {
        self != Ink::Background
    }
}

/// One drawing instruction.
#[derive(Clone, Copy, Debug)]
pub enum Op {
    /// Paint the whole panel.
    Clear { ink: Ink },
    /// One pixel.
    #[allow(dead_code, reason = "op vocabulary (ADR-0004), kept for the RA8875 backend")]
    Pix { x: i32, y: i32, ink: Ink },
    /// Straight line between two points, 1 px wide.
    Line { x0: i32, y0: i32, x1: i32, y1: i32, ink: Ink },
    /// Rectangle. `fill` chooses outline (thickness `t`) or solid.
    Rect {
        x: i32,
        y: i32,
        w: u16,
        h: u16,
        fill: bool,
        t: u16,
        ink: Ink,
    },
    /// Move the pixels of a window left by whole pixels, dropping the leftmost columns and
    /// vacating the right. Pixels outside the window do not move. Hardware scroll window on the
    /// RA8875; a bit loop on the shim.
    #[allow(dead_code, reason = "op vocabulary (ADR-0004), kept for the RA8875 backend")]
    ShiftBand {
        left: u16,
        top: u16,
        width: u16,
        height: u16,
        by: usize,
    },
    /// One character from the glyph atlas, its ink box at (x, y). Only lit pixels are drawn, at
    /// level `ink`; the rest of the box is left as it is. An atlas blit on the RA8875; a bit loop on
    /// the shim.
    Glyph { x: i32, y: i32, glyph: &'static Glyph, ink: Ink },
    /// Backlight, 0 to 100. The scaffold has no backlight, so it ignores this.
    #[allow(dead_code, reason = "op vocabulary (ADR-0004), kept for the RA8875 backend")]
    Brightness { percent: u8 },
}

/// What every panel backend must answer.
pub trait Panel {
    /// Panel size in pixels, which the layout derives everything from.
    fn size(&self) -> (u16, u16);
    /// Run one operation. Cheap on the RA8875, a memory write on the shim.
    fn apply(&mut self, op: Op);
    /// Get the accumulated operations onto the glass. This is the expensive step, and the one the
    /// frame budget is spent on.
    fn present(&mut self);
}
