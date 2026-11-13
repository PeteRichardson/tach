//! The chart model: each row's history as fixed spans of Mac time (ADR-0002).
//!
//! A bucket is one span of `BUCKET_MS`, about one pixel of travel. It keeps the lowest, highest
//! and newest value of the samples whose stamps fall in its span. So a peak between two frames
//! still reaches the glass (issue #9), and nothing is invented: a span with no sample stays empty,
//! and a gap longer than `GAP_MS` shows as a break in the curve.
//!
//! The curve is drawn from the model on every frame, at the exact Mac time of that frame. Motion
//! comes from the clock, not from the arrival of samples.

use crate::layout::{Row, GRID_TICK_S, TRAVEL_PX_PER_S};
use crate::ops::Op;
use crate::wire::Reply;

/// Travel, in pixels per millisecond of Mac time.
const PX_PER_MS: f32 = TRAVEL_PX_PER_S / 1000.0;
/// One bucket is one pixel of travel: 83 ms at 12 px/s.
pub const BUCKET_MS: u64 = (1000.0 / TRAVEL_PX_PER_S) as u64;
/// Buckets kept per row. More than the widest plot (378 px on the 480 px panel), so the curve
/// reaches the left edge.
pub const BUCKETS: usize = 400;
/// The longest gap the curve bridges with a line. GPU samples come about 6.5 times a second, so
/// this is several missed samples: a stalled source shows as a break, not as a flat line.
pub const GAP_MS: u64 = 1500;

/// Row order on the panel.
pub const CPU: usize = 0;
pub const GPU: usize = 1;
pub const MEM: usize = 2;

/// The lowest, highest and newest value in one span of time, 0 to 100.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bucket {
    /// Mac time of the span divided by `BUCKET_MS`. `EMPTY` before the slot is first used.
    span: u64,
    pub lo: f32,
    pub hi: f32,
    pub last: f32,
}

const EMPTY: u64 = u64::MAX;

/// One row's history: a ring of buckets, indexed by span.
pub struct Series {
    buckets: [Bucket; BUCKETS],
    /// Stamp of the newest sample taken, to drop a sample the Mac sends again.
    last_t: Option<u64>,
}

impl Series {
    pub const fn new() -> Self {
        Self {
            buckets: [Bucket { span: EMPTY, lo: 0.0, hi: 0.0, last: 0.0 }; BUCKETS],
            last_t: None,
        }
    }

    /// Add a sample stamped `t` (Mac ms). `lo` and `hi` are the extremes since the previous
    /// sample, if the source has them; otherwise pass `last` for both.
    ///
    /// A stamp that did not move is the same sample again and is dropped: the Mac sends the
    /// newest value in every reply, but GPU and memory change more slowly than the frame rate.
    pub fn add(&mut self, t: u64, last: f32, lo: f32, hi: f32) {
        if let Some(prev) = self.last_t {
            if t <= prev {
                // A step back longer than the history means the Mac clock started again (a
                // reboot). Anything shorter is a late or repeated sample.
                if prev - t <= BUCKETS as u64 * BUCKET_MS {
                    return;
                }
                *self = Self::new();
            }
        }
        self.last_t = Some(t);
        let span = t / BUCKET_MS;
        let b = &mut self.buckets[(span % BUCKETS as u64) as usize];
        if b.span == span {
            b.lo = b.lo.min(lo);
            b.hi = b.hi.max(hi);
            b.last = last;
        } else {
            *b = Bucket { span, lo, hi, last };
        }
    }

    /// The bucket for `span`, if a sample fell in it.
    pub fn bucket(&self, span: u64) -> Option<Bucket> {
        let b = self.buckets[(span % BUCKETS as u64) as usize];
        (b.span == span).then_some(b)
    }

    /// The newest value taken, for the telemetry line.
    pub fn newest(&self) -> Option<f32> {
        self.bucket(self.last_t? / BUCKET_MS).map(|b| b.last)
    }
}

/// The three rows, in panel order.
pub struct Model {
    pub rows: [Series; 3],
}

impl Model {
    pub const fn new() -> Self {
        Self { rows: [Series::new(), Series::new(), Series::new()] }
    }

    /// Take the values of one reply. A value without its stamp is not used: the stamp places it.
    pub fn take(&mut self, r: &Reply) {
        if let (Some(v), Some(t)) = (r.cpu, r.tc) {
            self.rows[CPU].add(t, v, v, v);
        }
        if let (Some(v), Some(t)) = (r.gpu, r.tg) {
            self.rows[GPU].add(t, v, v, v);
        }
        // Memory goes on the chart as a percent of the Mac's total, so all rows share one scale.
        if let (Some(gb), Some(t), Some(total)) = (r.mem, r.tm, r.mem_total) {
            if total > 0.0 {
                let pct = |g: f32| 100.0 * g / total;
                let (lo, hi) = (r.mem_lo.unwrap_or(gb), r.mem_hi.unwrap_or(gb));
                self.rows[MEM].add(t, pct(gb), pct(lo), pct(hi));
            }
        }
    }
}

/// Draw `series` into the plot area of `row` as it stands at Mac time `now`: the plot is cleared,
/// "now" is the right edge, and older times sit further left at `TRAVEL_PX_PER_S`. Each bucket
/// is a vertical stroke from its low to its high, joined to the next bucket by its newest value.
/// A short tick marks each `GRID_TICK_S` of Mac time, so the ruler travels with the curve.
pub fn draw(series: &Series, row: &Row, now: u64, mut op: impl FnMut(Op)) {
    let (left, top, w, h) = row.plot();
    op(Op::Rect { x: left as i32, y: top as i32, w, h, fill: true, t: 0, on: false });
    let left = left as i32;
    let right = left + w as i32 - 1;
    // Rounded to the nearest pixel. A stamp later than `now` (clock jitter) sits on the edge.
    let x_at = |t: u64| right - (now.saturating_sub(t) as f32 * PX_PER_MS + 0.5) as i32;
    let y_at = |v: f32| row.plot_y(v) as i32;

    let tick_ms = (GRID_TICK_S * 1000.0) as u64;
    let mut tick = now - now % tick_ms;
    loop {
        let x = x_at(tick);
        if x < left {
            break;
        }
        op(Op::Line { x0: x, y0: top as i32, x1: x, y1: top as i32 + 4, on: true });
        let Some(earlier) = tick.checked_sub(tick_ms) else { break };
        tick = earlier;
    }

    // Newest first, so each bucket can join the one to its right.
    let now_span = now / BUCKET_MS;
    let mut newer: Option<(u64, i32, i32)> = None; // span, x, y of its newest value
    for back in 0..BUCKETS as u64 {
        let Some(span) = now_span.checked_sub(back) else { break };
        let Some(b) = series.bucket(span) else { continue };
        let x = x_at(span * BUCKET_MS + BUCKET_MS / 2).min(right);
        if x < left {
            break;
        }
        op(Op::Line { x0: x, y0: y_at(b.hi), x1: x, y1: y_at(b.lo), on: true });
        let y = y_at(b.last);
        if let Some((n_span, n_x, n_y)) = newer {
            if (n_span - span) * BUCKET_MS <= GAP_MS {
                op(Op::Line { x0: x, y0: y, x1: n_x, y1: n_y, on: true });
            }
        }
        newer = Some((span, x, y));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout;

    fn row() -> Row {
        layout::rows(400, 240)[0]
    }

    /// Every op `draw` issues for `series` at `now`, without the clearing Rect.
    fn ops(series: &Series, now: u64) -> Vec<Op> {
        let mut out = Vec::new();
        draw(series, &row(), now, |op| out.push(op));
        assert!(matches!(out[0], Op::Rect { fill: true, on: false, .. }), "draw starts by clearing");
        out.remove(0);
        out
    }

    /// Lines that are not vertical: the joins between buckets.
    fn joins(ops: &[Op]) -> Vec<(i32, i32)> {
        ops.iter()
            .filter_map(|op| match *op {
                Op::Line { x0, x1, .. } if x0 != x1 => Some((x0, x1)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_bucket_keeps_low_high_and_newest() {
        let mut s = Series::new();
        let t = 10 * BUCKET_MS;
        s.add(t, 50.0, 50.0, 50.0);
        s.add(t + 10, 20.0, 20.0, 20.0);
        s.add(t + 20, 30.0, 25.0, 90.0);
        let b = s.bucket(10).unwrap();
        assert_eq!((b.lo, b.hi, b.last), (20.0, 90.0, 30.0));
        assert_eq!(s.newest(), Some(30.0));
    }

    #[test]
    fn a_repeated_stamp_is_dropped() {
        let mut s = Series::new();
        s.add(1000, 50.0, 50.0, 50.0);
        s.add(1000, 90.0, 90.0, 90.0);
        s.add(999, 10.0, 10.0, 10.0);
        assert_eq!(s.newest(), Some(50.0));
        assert_eq!(s.bucket(1000 / BUCKET_MS).unwrap().hi, 50.0);
    }

    #[test]
    fn a_mac_clock_restart_starts_the_history_again() {
        let mut s = Series::new();
        s.add(1_000_000, 50.0, 50.0, 50.0);
        s.add(5_000, 10.0, 10.0, 10.0);
        assert_eq!(s.bucket(1_000_000 / BUCKET_MS), None);
        assert_eq!(s.newest(), Some(10.0));
    }

    #[test]
    fn an_old_span_is_overwritten_by_a_new_one_in_its_slot() {
        let mut s = Series::new();
        s.add(0, 50.0, 50.0, 50.0);
        s.add(BUCKETS as u64 * BUCKET_MS, 10.0, 10.0, 10.0);
        assert_eq!(s.bucket(0), None);
        assert_eq!(s.bucket(BUCKETS as u64).unwrap().last, 10.0);
    }

    #[test]
    fn the_newest_sample_is_at_the_right_edge_and_older_ones_travel_left() {
        let (left, _, w, _) = row().plot();
        let right = (left + w - 1) as i32;
        let mut s = Series::new();
        let now = 100_000;
        s.add(now - 1000 - BUCKET_MS / 2, 50.0, 50.0, 50.0); // one second older
        s.add(now - BUCKET_MS / 2, 50.0, 50.0, 50.0); // centre of the newest span
        let strokes: Vec<i32> = ops(&s, now)
            .iter()
            .filter_map(|op| match *op {
                Op::Line { x0, x1, y0, .. } if x0 == x1 && y0 != row().plot().1 as i32 => Some(x0),
                _ => None,
            })
            .collect();
        // 12 px per second, within the rounding of a span to its centre.
        assert_eq!(strokes.len(), 2);
        assert!((strokes[0] - right).abs() <= 1, "{strokes:?}");
        assert!((strokes[1] - (right - 12)).abs() <= 1, "{strokes:?}");
    }

    #[test]
    fn a_gap_longer_than_gap_ms_breaks_the_curve() {
        let mut s = Series::new();
        let now = 100_000;
        // Oldest first, as the Mac sends them: a long gap, then a short one.
        s.add(now - 100 - GAP_MS / 2 - 2 * GAP_MS, 50.0, 50.0, 50.0);
        s.add(now - 100 - GAP_MS / 2, 50.0, 50.0, 50.0);
        s.add(now - 100, 50.0, 50.0, 50.0);
        assert_eq!(joins(&ops(&s, now)).len(), 1);
    }

    #[test]
    fn nothing_is_drawn_outside_the_plot_area() {
        let (left, top, w, h) = row().plot();
        let mut s = Series::new();
        let now = 1_000_000;
        // Older than the plot is wide, newer than now, and both extremes of the scale.
        for i in 0..(BUCKETS as u64 * 2) {
            let v = if i % 2 == 0 { 0.0 } else { 100.0 };
            s.add(now - 60_000 + i * 60, v, v, v);
        }
        s.add(now + 500, 200.0, -5.0, 200.0);
        for op in ops(&s, now) {
            let Op::Line { x0, y0, x1, y1, .. } = op else { panic!("unexpected {op:?}") };
            for (x, y) in [(x0, y0), (x1, y1)] {
                assert!(x >= left as i32 && x < (left + w) as i32, "x {x} in {op:?}");
                assert!(y >= top as i32 && y < (top + h) as i32, "y {y} in {op:?}");
            }
        }
    }

    #[test]
    fn grid_ticks_sit_on_whole_multiples_of_mac_time() {
        let s = Series::new();
        let (left, _, w, _) = row().plot();
        let right = (left + w - 1) as i32;
        let now = 12_500; // ticks at 10 s, 5 s and 0 s: 2.5, 7.5 and 12.5 s ago
        let ticks: Vec<i32> = ops(&s, now)
            .iter()
            .map(|op| match *op {
                Op::Line { x0, .. } => x0,
                _ => panic!(),
            })
            .collect();
        assert_eq!(ticks, vec![right - 30, right - 90, right - 150]);
    }

    #[test]
    fn the_model_puts_memory_on_the_chart_as_a_percent_of_the_total() {
        let mut m = Model::new();
        let r = crate::wire::parse(
            br#"{"pull":1,"v":2,"t":1000,"cpu":12.5,"tc":999,"mem":32.0,"mem_lo":16.0,"mem_hi":64.0,"tm":990,"mem_total":128.0,"ok":false}"#,
        )
        .unwrap();
        m.take(&r);
        assert_eq!(m.rows[CPU].newest(), Some(12.5));
        assert_eq!(m.rows[GPU].newest(), None);
        let b = m.rows[MEM].bucket(990 / BUCKET_MS).unwrap();
        assert_eq!((b.lo, b.hi, b.last), (12.5, 50.0, 25.0));
    }
}
