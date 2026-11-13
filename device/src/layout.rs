//! Geometry, derived from the panel size. One place, so 400x240 today and 480x272 next week are
//! two constant blocks and not a rewrite. See docs/device-v2.md, Settled parameters.

/// Width of the number slot on the left of a row, in pixels (Q34: the v1 slot width).
pub const NUMBER_SLOT_PX: u16 = 114;
/// Margin around the row content, in pixels.
pub const MARGIN_PX: u16 = 4;
/// Number of rows on screen (Q37: three, bigger beats more).
pub const ROWS: usize = 3;
/// Chart travel, pixels per second. Chosen so that a sample at 8 Hz lands 1.5 px apart, which is
/// 0.4 px per frame at 30 fps — smooth with sub-pixel placement.
pub const TRAVEL_PX_PER_S: f32 = 12.0;
/// Seconds between grid ticks on the time axis.
pub const GRID_TICK_S: f32 = 5.0;
/// Height of the axis label strip under a chart, in pixels.
pub const AXIS_PX: u16 = 10;

/// A row's rectangle inside the panel, in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    /// Left edge of the number slot.
    pub slot_x: u16,
    /// Left edge of the chart, which is the oldest visible time.
    pub chart_x: u16,
    /// Right edge of the chart, which is "now".
    pub chart_right: u16,
    /// Top edge of the chart.
    pub chart_y: u16,
    /// Height of the chart.
    pub chart_h: u16,
    /// Panel width this row was computed for, kept so a wrong panel size is visible in a dump.
    pub panel_w: u16,
}

impl Row {
    /// Chart width in pixels. History in seconds is this divided by `TRAVEL_PX_PER_S`: 25 s on a
    /// 400 px panel, 32 s on a 480 px panel.
    pub fn chart_w(&self) -> u16 {
        self.chart_right - self.chart_x
    }

    /// The plot area: the inside of the 1 px chart frame, as (left, top, width, height). The curve
    /// is drawn and scrolled only in here, so the frame itself never moves.
    pub fn plot(&self) -> (u16, u16, u16, u16) {
        (self.chart_x + 1, self.chart_y + 1, self.chart_w() - 2, self.chart_h - 2)
    }

    /// The number slot: from the slot's left edge to the margin before the chart frame, and as high
    /// as the chart, as (left, top, width, height).
    pub fn slot(&self) -> (u16, u16, u16, u16) {
        (self.slot_x, self.chart_y, self.chart_x - MARGIN_PX - self.slot_x, self.chart_h)
    }

    /// Baseline of the row label: its capitals (12 px at 16 px) sit level with the chart's top.
    pub fn label_baseline(&self) -> u16 {
        self.chart_y + 12
    }

    /// Baseline of the number: on the chart's bottom frame line.
    pub fn number_baseline(&self) -> u16 {
        self.chart_y + self.chart_h - 1
    }

    /// Right end of the number's pen travel: the right edge of the slot.
    pub fn number_right(&self) -> u16 {
        self.chart_x - MARGIN_PX
    }

    /// Plot row for a value from 0 to 100: 0 on the bottom row of the plot area, 100 on the top.
    pub fn plot_y(&self, v: f32) -> u16 {
        let (_, top, _, h) = self.plot();
        top + h - 1 - ((v.clamp(0.0, 100.0) / 100.0) * (h - 1) as f32) as u16
    }
}

/// Rows for a panel of the given size, laid out top to bottom.
pub fn rows(panel_w: u16, panel_h: u16) -> [Row; ROWS] {
    let row_h = panel_h / ROWS as u16;
    let chart_x = NUMBER_SLOT_PX + MARGIN_PX;
    let chart_right = panel_w - MARGIN_PX;
    let chart_h = row_h - AXIS_PX - MARGIN_PX * 2;
    [0, 1, 2].map(|i| Row {
        slot_x: MARGIN_PX,
        chart_x,
        chart_right,
        chart_y: i as u16 * row_h + MARGIN_PX,
        chart_h,
        panel_w,
    })
}

/// Whole-pixel spacing of grid ticks, for the fixed-point arithmetic in the frame loop.
pub const GRID_TICK_PX_X100: u32 = (TRAVEL_PX_PER_S * GRID_TICK_S * 100.0) as u32;
