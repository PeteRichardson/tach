//! The drawing operations the firmware issues, and the contract every panel backend must meet.
//!
//! The firmware never builds a whole frame and ships it. It issues operations; each backend does
//! what its controller does best. See ADR-0004.

/// Which corner of the panel maps to (0,0). The RA8875 has a MADR-style register; the shim only
/// needs it so that a rotated target panel can be previewed on the scaffold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Madrs {
    /// (0,0) top-left.
    Normal,
    /// (0,0) top-right.
    MirrorX,
    /// (0,0) bottom-left.
    MirrorY,
    /// (0,0) bottom-right.
    MirrorXY,
}

/// One drawing instruction.
#[derive(Clone, Copy, Debug)]
pub enum Op {
    /// Paint the whole panel. `on` is a level, not a colour: the backend maps it through the
    /// level table.
    Clear { on: bool },
    /// One pixel.
    Pix { x: i32, y: i32, on: bool },
    /// Straight line between two points, 1 px wide.
    Line { x0: i32, y0: i32, x1: i32, y1: i32, on: bool },
    /// Rectangle. `fill` chooses outline (thickness `t`) or solid.
    Rect {
        x: i32,
        y: i32,
        w: u16,
        h: u16,
        fill: bool,
        t: u16,
        on: bool,
    },
    /// Move the pixels of a window left by whole pixels, dropping the leftmost columns and
    /// vacating the right. Pixels outside the window do not move. Hardware scroll window on the
    /// RA8875; a bit loop on the shim.
    ShiftBand {
        left: u16,
        top: u16,
        width: u16,
        height: u16,
        by: usize,
    },
    /// Backlight, 0 to 100. The scaffold has no backlight, so it ignores this.
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
