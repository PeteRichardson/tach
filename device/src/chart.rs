//! The chart model: each row's history as fixed spans of Mac time (ADR-0002).
//!
//! A bucket is one span of `BUCKET_MS`, about one pixel of travel. It keeps the lowest, highest
//! and newest value of the samples whose stamps fall in its span. So a peak between two frames
//! still reaches the glass (issue #9), and nothing is invented: a span with no sample stays empty,
//! and a gap longer than `GAP_MS` shows as a break in the curve.
//!
//! The curve is drawn from the model on every frame, at the exact Mac time of that frame. Motion
//! comes from the clock, not from the arrival of samples.
//!
//! CPU and GPU use a linear axis of `TRAVEL_PX_PER_S`. Memory uses a compressed axis of 5 minutes
//! (design.md 5.2, `Axis`): its right edge travels at the same speed, so one speed holds across
//! the screen, and further left each column covers more time.

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
/// The memory row's window: the age of its left edge.
pub const MEM_WINDOW_MS: u64 = 300_000;
/// Buckets kept for memory: the whole window at one bucket per pixel of right-edge travel.
pub const MEM_BUCKETS: usize = (MEM_WINDOW_MS / BUCKET_MS) as usize + 1;
/// Marks on the memory row at 1, 2, 3 and 4 minutes ago. They stay where they are: on a
/// compressed axis a mark that travelled would change speed as it went.
pub const MEM_MARKS_MS: [u64; 4] = [60_000, 120_000, 180_000, 240_000];
/// The widest plot an `Axis` serves. The 400 px panel has a 276 px plot.
const MAX_PLOT_W: usize = 400;

/// The longest gap the curve bridges with a line. GPU samples come about 6.5 times a second, so
/// this is several missed samples: a stalled source shows as a break, not as a flat line.
pub const GAP_MS: u64 = 1500;

/// The staleness limit (GLOSSARY, "Stale"), one limit for both halves of the rule:
///
/// * A row whose newest sample is older than this, by Mac time, is stale: a dead source must not
///   look live (issue #1).
/// * When no reply came for this long, there is no Mac time to draw at, and the panel blanks
///   instead of freezing (ADR-0005, ADR-0006).
///
/// 5 s, the same as v1 (`STALE_NS` in code.py) and macmon's limit in tach-sender.py
/// (`MACMON_STALE_S`), so all three say one thing (issue #27).
pub const STALE_MS: u64 = 5000;

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

/// One row's history: a ring of `N` buckets, indexed by span.
pub struct Series<const N: usize> {
    buckets: [Bucket; N],
    /// Stamp of the newest sample taken, to drop a sample the Mac sends again.
    last_t: Option<u64>,
}

impl<const N: usize> Series<N> {
    pub const fn new() -> Self {
        Self {
            buckets: [Bucket { span: EMPTY, lo: 0.0, hi: 0.0, last: 0.0 }; N],
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
                if prev - t <= N as u64 * BUCKET_MS {
                    return;
                }
                *self = Self::new();
            }
        }
        self.last_t = Some(t);
        let span = t / BUCKET_MS;
        let b = &mut self.buckets[(span % N as u64) as usize];
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
        let b = self.buckets[(span % N as u64) as usize];
        (b.span == span).then_some(b)
    }

    /// True if the newest sample is older than `STALE_MS` at Mac time `now`, or if there is none.
    pub fn is_stale(&self, now: u64) -> bool {
        self.last_t.map_or(true, |t| now.saturating_sub(t) > STALE_MS)
    }

    /// The newest value taken, for the telemetry line.
    pub fn newest(&self) -> Option<f32> {
        self.bucket(self.last_t? / BUCKET_MS).map(|b| b.last)
    }
}

/// The history of a CPU or GPU row.
pub type Fast = Series<BUCKETS>;
/// The history of the memory row: 5 minutes.
pub type Slow = Series<MEM_BUCKETS>;

/// The three rows.
pub struct Model {
    pub cpu: Fast,
    pub gpu: Fast,
    pub mem: Slow,
    /// Newest memory used in GB, for the number slot: the chart shows a percent, the number GB.
    pub mem_gb: Option<f32>,
}

impl Model {
    pub const fn new() -> Self {
        Self { cpu: Fast::new(), gpu: Fast::new(), mem: Slow::new(), mem_gb: None }
    }

    /// True if row `i` (`CPU`, `GPU` or `MEM`) is stale at Mac time `now`.
    pub fn is_stale(&self, i: usize, now: u64) -> bool {
        match i {
            CPU => self.cpu.is_stale(now),
            GPU => self.gpu.is_stale(now),
            _ => self.mem.is_stale(now),
        }
    }

    /// Take the values of one reply. A value without its stamp is not used: the stamp places it.
    pub fn take(&mut self, r: &Reply) {
        if let (Some(v), Some(t)) = (r.cpu, r.tc) {
            self.cpu.add(t, v, v, v);
        }
        if let (Some(v), Some(t)) = (r.gpu, r.tg) {
            self.gpu.add(t, v, v, v);
        }
        // Memory goes on the chart as a percent of the Mac's total, so all rows share one scale.
        if let (Some(gb), Some(t), Some(total)) = (r.mem, r.tm, r.mem_total) {
            if total > 0.0 {
                let pct = |g: f32| 100.0 * g / total;
                let (lo, hi) = (r.mem_lo.unwrap_or(gb), r.mem_hi.unwrap_or(gb));
                self.mem.add(t, pct(gb), pct(lo), pct(hi));
                self.mem_gb = Some(gb);
            }
        }
    }
}

/// Mac time for a frame at device time `now_us`, or None when there is nothing to draw at.
///
/// `offset_ms` is Mac ms minus device ms from the newest reply, and `last_reply_us` is the device
/// time of that reply. With no reply yet, or none for longer than `STALE_MS`, this is None and
/// the caller blanks the plots: the device clock would keep the curves moving, and an old curve
/// that moves looks live.
pub fn mac_now(offset_ms: Option<i64>, last_reply_us: Option<u64>, now_us: u64) -> Option<u64> {
    let last = last_reply_us?;
    if now_us.saturating_sub(last) > STALE_MS * 1000 {
        return None;
    }
    Some(((now_us / 1000) as i64 + offset_ms?) as u64)
}

/// Clear the plot area of `row`: what a row shows when the link is stale.
pub fn blank(row: &Row, mut op: impl FnMut(Op)) {
    let (left, top, w, h) = row.plot();
    op(Op::Rect { x: left as i32, y: top as i32, w, h, fill: true, t: 0, on: false });
}

/// Draw `series` into the plot area of `row` as it stands at Mac time `now`: the plot is cleared,
/// "now" is the right edge, and older times sit further left at `TRAVEL_PX_PER_S`. Each bucket
/// is a vertical stroke from its low to its high, joined to the next bucket by its newest value.
/// A short tick marks each `GRID_TICK_S` of Mac time, so the ruler travels with the curve.
pub fn draw(series: &Fast, row: &Row, now: u64, mut op: impl FnMut(Op)) {
    blank(row, &mut op);
    let (left, top, w, _) = row.plot();
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

/// The memory row's compressed time axis (design.md 5.2). Column `x` counts from the right edge,
/// and its age grows as `b * (e^(x / b) - 1)` pixels of travel: one pixel per pixel at the right
/// edge, so the newest memory travels at `TRAVEL_PX_PER_S` like the other rows, and `b` is chosen
/// so the left edge is `MEM_WINDOW_MS` old. On the 400 px panel the left column covers about 4 s.
pub struct Axis {
    /// `edges[x]` is the age in ms where column `x` starts; column `x` ends at `edges[x + 1]`.
    edges: [u32; MAX_PLOT_W + 1],
    w: usize,
}

impl Axis {
    /// The axis for a plot `w` pixels wide. Runs once at start-up: 60 steps of bisection.
    pub fn new(w: u16) -> Self {
        let w = (w as usize).clamp(1, MAX_PLOT_W);
        let window_px = MEM_WINDOW_MS as f32 * PX_PER_MS;
        let age_px = |x: f32, b: f32| b * (libm::expf(x / b) - 1.0);
        // age_px(w, b) gets smaller as b grows, down to w at b = infinity.
        let (mut lo, mut hi) = (1.0f32, 1.0e6f32);
        for _ in 0..60 {
            let b = (lo + hi) / 2.0;
            if age_px(w as f32, b) > window_px {
                lo = b;
            } else {
                hi = b;
            }
        }
        let b = (lo + hi) / 2.0;
        let mut edges = [0u32; MAX_PLOT_W + 1];
        for x in 1..w {
            let ms = (age_px(x as f32, b) / PX_PER_MS + 0.5) as u32;
            edges[x] = ms.max(edges[x - 1] + 1);
        }
        edges[w] = (MEM_WINDOW_MS as u32).max(edges[w - 1] + 1);
        Self { edges, w }
    }

    /// The column, from the right edge, that shows a sample `age_ms` old. None if it is older than
    /// the window.
    pub fn column(&self, age_ms: u64) -> Option<usize> {
        let x = self.edges[..=self.w].partition_point(|&e| e as u64 <= age_ms);
        (x <= self.w).then(|| x - 1)
    }
}

/// The buckets that fall in one column of the compressed axis, merged.
#[derive(Clone, Copy)]
struct Column {
    x: usize,
    lo: f32,
    hi: f32,
    /// The newest value in the column.
    last: f32,
    newest_t: u64,
    oldest_t: u64,
}

/// Draw the memory row on its compressed axis: as `draw`, but each column is a stroke from the
/// lowest to the highest value of all its buckets, so a short peak far back in the 5 minutes
/// still shows (GLOSSARY, "Peak keep"). Two columns join when they are next to each other or when
/// less than `GAP_MS` lies between them; a longer gap breaks the curve.
pub fn draw_compressed(series: &Slow, row: &Row, axis: &Axis, now: u64, mut op: impl FnMut(Op)) {
    blank(row, &mut op);
    let (left, top, w, _) = row.plot();
    let right = left as i32 + w as i32 - 1;
    let y_at = |v: f32| row.plot_y(v) as i32;

    for age in MEM_MARKS_MS {
        if let Some(x) = axis.column(age) {
            let x = right - x as i32;
            op(Op::Line { x0: x, y0: top as i32, x1: x, y1: top as i32 + 4, on: true });
        }
    }

    let mut newer: Option<Column> = None;
    let mut stroke = |c: Column| {
        let x = right - c.x as i32;
        op(Op::Line { x0: x, y0: y_at(c.hi), x1: x, y1: y_at(c.lo), on: true });
        if let Some(n) = newer {
            if c.x - n.x == 1 || n.oldest_t - c.newest_t <= GAP_MS {
                op(Op::Line { x0: x, y0: y_at(c.last), x1: right - n.x as i32, y1: y_at(n.last), on: true });
            }
        }
        newer = Some(c);
    };

    // Newest first. A column is drawn when the first bucket of an older column arrives. This walks
    // up to 3600 buckets on each frame, so it keeps a ring index and a column cursor: no 64-bit
    // division and no search per bucket.
    let now_span = now / BUCKET_MS;
    let mut slot = (now_span % MEM_BUCKETS as u64) as usize;
    let mut x = 0;
    let mut col: Option<Column> = None;
    for back in 0..MEM_BUCKETS as u64 {
        let Some(span) = now_span.checked_sub(back) else { break };
        let b = series.buckets[slot];
        slot = if slot == 0 { MEM_BUCKETS - 1 } else { slot - 1 };
        if b.span != span {
            continue;
        }
        let t = span * BUCKET_MS + BUCKET_MS / 2;
        let age = now.saturating_sub(t);
        while x < axis.w && age >= axis.edges[x + 1] as u64 {
            x += 1;
        }
        if x == axis.w {
            break;
        }
        match &mut col {
            Some(c) if c.x == x => {
                c.lo = c.lo.min(b.lo);
                c.hi = c.hi.max(b.hi);
                c.oldest_t = t;
            }
            _ => {
                if let Some(c) = col.take() {
                    stroke(c);
                }
                col = Some(Column { x, lo: b.lo, hi: b.hi, last: b.last, newest_t: t, oldest_t: t });
            }
        }
    }
    if let Some(c) = col {
        stroke(c);
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
    fn ops(series: &Fast, now: u64) -> Vec<Op> {
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
        let mut s = Fast::new();
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
        let mut s = Fast::new();
        s.add(1000, 50.0, 50.0, 50.0);
        s.add(1000, 90.0, 90.0, 90.0);
        s.add(999, 10.0, 10.0, 10.0);
        assert_eq!(s.newest(), Some(50.0));
        assert_eq!(s.bucket(1000 / BUCKET_MS).unwrap().hi, 50.0);
    }

    #[test]
    fn a_mac_clock_restart_starts_the_history_again() {
        let mut s = Fast::new();
        s.add(1_000_000, 50.0, 50.0, 50.0);
        s.add(5_000, 10.0, 10.0, 10.0);
        assert_eq!(s.bucket(1_000_000 / BUCKET_MS), None);
        assert_eq!(s.newest(), Some(10.0));
    }

    #[test]
    fn an_old_span_is_overwritten_by_a_new_one_in_its_slot() {
        let mut s = Fast::new();
        s.add(0, 50.0, 50.0, 50.0);
        s.add(BUCKETS as u64 * BUCKET_MS, 10.0, 10.0, 10.0);
        assert_eq!(s.bucket(0), None);
        assert_eq!(s.bucket(BUCKETS as u64).unwrap().last, 10.0);
    }

    #[test]
    fn the_newest_sample_is_at_the_right_edge_and_older_ones_travel_left() {
        let (left, _, w, _) = row().plot();
        let right = (left + w - 1) as i32;
        let mut s = Fast::new();
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
        let mut s = Fast::new();
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
        let mut s = Fast::new();
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
        let s = Fast::new();
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
    fn a_row_is_stale_when_its_newest_sample_is_older_than_the_limit() {
        let mut s = Fast::new();
        assert!(s.is_stale(10_000), "no sample yet");
        s.add(10_000, 50.0, 50.0, 50.0);
        assert!(!s.is_stale(10_000 + STALE_MS));
        assert!(s.is_stale(10_000 + STALE_MS + 1));
    }

    #[test]
    fn a_repeated_stamp_does_not_keep_a_row_fresh() {
        // The Mac sends GPU in every reply, with the stamp macmon gave it. When macmon stops, the
        // stamp stops too, and the row must go stale while replies still come in.
        let mut s = Fast::new();
        s.add(10_000, 50.0, 50.0, 50.0);
        for _ in 0..10 {
            s.add(10_000, 50.0, 50.0, 50.0);
        }
        assert!(s.is_stale(10_000 + STALE_MS + 1));
    }

    #[test]
    fn mac_time_follows_the_device_clock_from_the_last_reply() {
        // Reply at device 2.000 s carried Mac t = 1_000_000 ms: offset 998_000.
        assert_eq!(mac_now(Some(998_000), Some(2_000_000), 2_500_000), Some(1_000_500));
    }

    #[test]
    fn no_mac_time_before_the_first_reply_or_after_the_link_goes_stale() {
        assert_eq!(mac_now(None, None, 1_000_000), None);
        let last = 2_000_000;
        assert!(mac_now(Some(0), Some(last), last + STALE_MS * 1000).is_some());
        assert_eq!(mac_now(Some(0), Some(last), last + STALE_MS * 1000 + 1), None);
    }

    #[test]
    fn the_model_puts_memory_on_the_chart_as_a_percent_of_the_total() {
        let mut m = Model::new();
        let r = crate::wire::parse(
            br#"{"pull":1,"v":2,"t":1000,"cpu":12.5,"tc":999,"mem":32.0,"mem_lo":16.0,"mem_hi":64.0,"tm":990,"mem_total":128.0,"ok":false}"#,
        )
        .unwrap();
        m.take(&r);
        assert_eq!(m.cpu.newest(), Some(12.5));
        assert_eq!(m.gpu.newest(), None);
        let b = m.mem.bucket(990 / BUCKET_MS).unwrap();
        assert_eq!((b.lo, b.hi, b.last), (12.5, 50.0, 25.0));
    }

    fn mem_row() -> Row {
        layout::rows(400, 240)[MEM]
    }

    /// The x of each stroke (vertical line below the marks) `draw_compressed` issues.
    fn mem_strokes(series: &Slow, now: u64) -> Vec<(i32, i32, i32)> {
        let row = mem_row();
        let axis = Axis::new(row.plot().2);
        let mut out = Vec::new();
        draw_compressed(series, &row, &axis, now, |op| {
            if let Op::Line { x0, y0, x1, y1, .. } = op {
                if x0 == x1 && !(y0 == row.plot().1 as i32 && y1 == y0 + 4) {
                    out.push((x0, y0.min(y1), y0.max(y1)));
                }
            }
        });
        out
    }

    #[test]
    fn the_memory_axis_spans_five_minutes_and_starts_at_the_common_speed() {
        let w = mem_row().plot().2;
        let axis = Axis::new(w);
        // One pixel at the right edge is one pixel of travel at 12 px/s: 83 ms.
        assert!((83..=84).contains(&axis.edges[1]), "{}", axis.edges[1]);
        assert_eq!(axis.edges[w as usize], MEM_WINDOW_MS as u32);
        assert!(axis.edges[..=w as usize].windows(2).all(|e| e[0] < e[1]));
        assert_eq!(axis.column(0), Some(0));
        assert_eq!(axis.column(MEM_WINDOW_MS - 1), Some(w as usize - 1));
        assert_eq!(axis.column(MEM_WINDOW_MS), None);
        // The left column covers a few seconds, not one sample.
        let left = axis.edges[w as usize] - axis.edges[w as usize - 1];
        assert!((2_000..8_000).contains(&left), "{left} ms");
    }

    #[test]
    fn new_memory_travels_like_the_other_rows() {
        let (left, _, w, _) = mem_row().plot();
        let right = (left + w - 1) as i32;
        let mut s = Slow::new();
        let now = 1_000_000;
        s.add(now - 1000 - BUCKET_MS / 2, 50.0, 50.0, 50.0);
        s.add(now - BUCKET_MS / 2, 50.0, 50.0, 50.0);
        let xs: Vec<i32> = mem_strokes(&s, now).iter().map(|s| s.0).collect();
        assert_eq!(xs.len(), 2);
        assert!((xs[0] - right).abs() <= 1, "{xs:?}");
        assert!((xs[1] - (right - 12)).abs() <= 1, "{xs:?}");
    }

    #[test]
    fn memory_four_minutes_back_is_still_on_the_chart() {
        let mut s = Slow::new();
        let now = 1_000_000;
        s.add(now - 240_000, 50.0, 50.0, 50.0);
        let xs = mem_strokes(&s, now);
        let (left, _, w, _) = mem_row().plot();
        assert_eq!(xs.len(), 1);
        // Far into the left half: the compressed part of the axis.
        assert!(xs[0].0 >= left as i32 && xs[0].0 < (left + w / 3) as i32, "{xs:?}");
    }

    #[test]
    fn a_short_old_memory_peak_survives_the_compression() {
        let mut s = Slow::new();
        let now = 1_000_000;
        // 10 Hz at 20 %, with one 100 ms sample at 90 % three minutes ago.
        let start = now - MEM_WINDOW_MS + 1000;
        for i in 0..(MEM_WINDOW_MS - 2000) / 100 {
            let t = start + i * 100;
            let v = if t == now - 180_000 { 90.0 } else { 20.0 };
            s.add(t, v, v, v);
        }
        let peak_y = mem_row().plot_y(90.0) as i32;
        assert!(mem_strokes(&s, now).iter().any(|&(_, top, _)| top == peak_y));
    }

    #[test]
    fn memory_strokes_stay_inside_the_plot_and_fill_it() {
        let (left, top, w, h) = mem_row().plot();
        let mut s = Slow::new();
        let now = 1_000_000;
        for i in 0..(MEM_WINDOW_MS + 20_000) / 50 {
            let v = (i % 101) as f32;
            s.add(now - MEM_WINDOW_MS - 20_000 + i * 50, v, v, v);
        }
        let xs = mem_strokes(&s, now);
        for &(x, y0, y1) in &xs {
            assert!(x >= left as i32 && x < (left + w) as i32, "x {x}");
            assert!(y0 >= top as i32 && y1 < (top + h) as i32);
        }
        // At 20 Hz every bucket has a sample, so every column has a stroke.
        assert_eq!(xs.len(), w as usize, "{:?}", (left, xs.first(), xs.last()));
    }

    #[test]
    fn the_memory_marks_sit_at_whole_minutes_and_stay_put() {
        let row = mem_row();
        let axis = Axis::new(row.plot().2);
        let marks = |now: u64| {
            let mut out = Vec::new();
            draw_compressed(&Slow::new(), &row, &axis, now, |op| {
                if let Op::Line { x0, .. } = op {
                    out.push(x0);
                }
            });
            out
        };
        assert_eq!(marks(1_000_000).len(), 4);
        assert_eq!(marks(1_000_000), marks(1_003_456));
    }
}
