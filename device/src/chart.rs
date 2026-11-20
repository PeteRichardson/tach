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
//! (docs/device-v2.md, "Settled parameters", and `Axis` below): its right edge travels at the same speed, so one speed holds across
//! the screen, and further left each column covers more time.

use crate::layout::{Row, GRID_TICK_S, TRAVEL_PX_PER_S};
use crate::ops::{Ink, Op};
use crate::wire::{Reply, MAX_CORES};

/// One bucket is one pixel of travel: 83 ms at 12 px/s.
pub const BUCKET_MS: u64 = (1000.0 / TRAVEL_PX_PER_S) as u64;
/// Travel, in pixels per millisecond of Mac time: exactly one pixel per bucket, so 12.05 px/s.
/// With `TRAVEL_PX_PER_S / 1000` the buckets were 0.996 px apart, each one stepped to its next
/// pixel 0.33 ms before the bucket to its right, and the steps rippled across the plot.
const PX_PER_MS: f32 = 1.0 / BUCKET_MS as f32;
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
/// 5 s, to match macmon's limit in tach-sender.py (`MACMON_STALE_S`), so the two live limits
/// say one thing (issue #27). v1 used the same 5 s as `STALE_NS`; v1 is retired.
pub const STALE_MS: u64 = 5000;

/// Time constant of a core bar's smoothing: short, so the bars move like a meter, not a chart.
pub const EMA_MS: f32 = 100.0;
/// How long a peak marker stays at its peak, then how fast it falls, in percent per second.
pub const HOLD_MS: u64 = 600;
pub const FALL_PER_S: f32 = 30.0;

#[derive(Clone, Copy)]
struct Bar {
    level: f32,
    peak: f32,
    /// When the peak was set, Mac ms.
    at: u64,
}

const NO_BAR: Bar = Bar { level: 0.0, peak: 0.0, at: 0 };

/// One bar for each core, like an audio spectrum analyser: a smoothed level, and a peak marker
/// that holds the highest raw value, then falls back to the level.
#[derive(Clone, Copy)]
pub struct Bars {
    bars: [Bar; MAX_CORES],
    n: u8,
    nsuper: u8,
    /// Mac time of the newest core sample.
    t: Option<u64>,
}

impl Bars {
    pub const fn new() -> Self {
        Self { bars: [NO_BAR; MAX_CORES], n: 0, nsuper: 0, t: None }
    }

    /// Take the core values of one reply. A sample at the same stamp as the last is a repeat.
    pub fn take(&mut self, r: &Reply) {
        let (Some(cores), Some(t)) = (r.cores, r.tc) else { return };
        let pct = cores.percents();
        if pct.len() != self.n as usize {
            // A new Mac, or the first sample: start each bar at its value.
            *self = Self::new();
            self.n = pct.len() as u8;
        }
        self.nsuper = r.nsuper.unwrap_or(0).min(self.n as u32) as u8;
        let k = match self.t {
            None => 1.0,
            Some(last) if t <= last => return,
            Some(last) => 1.0 - libm::expf(-((t - last) as f32) / EMA_MS),
        };
        self.t = Some(t);
        for (bar, &p) in self.bars.iter_mut().zip(pct) {
            let raw = p as f32;
            bar.level += (raw - bar.level) * k;
            // The fall is worked out when the peak is read, so only a new high changes it.
            if raw >= bar.peak_at(t) {
                (bar.peak, bar.at) = (raw, t);
            }
        }
    }

    pub fn len(&self) -> usize {
        self.n as usize
    }

    /// How many of the bars, from the left, are super cores.
    pub fn supers(&self) -> usize {
        self.nsuper as usize
    }

    /// The smoothed level of bar `i`, in percent.
    pub fn level(&self, i: usize) -> f32 {
        self.bars[i].level
    }

    /// Where the peak marker of bar `i` is at Mac time `now`, in percent.
    pub fn peak(&self, i: usize, now: u64) -> f32 {
        self.bars[i].peak_at(now)
    }
}

impl Bar {
    fn peak_at(&self, now: u64) -> f32 {
        let falling_ms = now.saturating_sub(self.at).saturating_sub(HOLD_MS);
        (self.peak - FALL_PER_S * falling_ms as f32 / 1000.0).max(self.level)
    }
}

/// A core bar's segments: `SEG_PX` high, one every `SEG_PITCH` px, so a 1 px gap between.
pub const SEG_PX: u16 = 3;
pub const SEG_PITCH: usize = 4;

/// How many segments a bar in `row` has: at most 31, so that a full bar's bits fit a u32 shift.
pub fn segs(row: &Row) -> usize {
    (row.plot().3 as usize / SEG_PITCH).min(31)
}

/// The core bars as they are on the panel, so a frame redraws only the segments that changed.
/// At 36 bars of 17 segments, drawing every segment was more than 600 rects a frame.
pub struct BarsView {
    /// Bit `s` of a bar is set when its segment `s` is lit. None when the plot does not show bars.
    lit: Option<[u32; MAX_CORES]>,
    /// How many bars were drawn. Another count moves every bar.
    cores: usize,
}

impl BarsView {
    pub const fn new() -> Self {
        Self { lit: None, cores: 0 }
    }

    /// The plot was drawn over: the next `draw` clears it and draws every segment.
    pub fn forget(&mut self) {
        self.lit = None;
    }

    /// Draw the CPU row at Mac time `now`: the core bars, or the CPU curve, as before, when the
    /// sender gives no per-core values.
    pub fn draw_cpu(&mut self, model: &Model, row: &Row, now: u64, op: impl FnMut(Op)) {
        if model.bars.len() == 0 {
            self.forget();
            draw(&model.cpu, row, now, op)
        } else {
            self.draw(&model.bars, row, now, op)
        }
    }

    /// Draw `bars` in `row`'s plot at Mac time `now`. They fill the plot's width, the super cores
    /// first. A bar is lit up to its level, and one more segment shows its peak.
    pub fn draw(&mut self, bars: &Bars, row: &Row, now: u64, mut op: impl FnMut(Op)) {
        let (left, top, w, h) = row.plot();
        let n = segs(row);
        let cores = bars.len();
        if cores != self.cores {
            (self.lit, self.cores) = (None, cores);
        }
        let all = self.lit.is_none();
        if all {
            blank(row, &mut op);
        }
        let lit = self.lit.get_or_insert([0; MAX_CORES]);
        let count = |v: f32| (v.clamp(0.0, 100.0) / 100.0 * n as f32 + 0.5) as usize;
        for (i, drawn) in lit.iter_mut().enumerate().take(cores) {
            let level = count(bars.level(i));
            let peak = count(bars.peak(i, now));
            let mut on = (1u32 << level) - 1;
            if peak > level {
                on |= 1 << (peak - 1);
            }
            let changed = if all { !0 } else { on ^ *drawn };
            *drawn = on;
            if changed & ((1 << n) - 1) == 0 {
                continue;
            }
            let kind = (i >= bars.supers()) as u8;
            let (x0, x1) = (i * w as usize / cores, (i + 1) * w as usize / cores);
            let (x, bar_w) = (left as i32 + x0 as i32, (x1 - x0) as u16 - 1);
            for s in (0..n).filter(|s| changed & (1 << s) != 0) {
                let ink = match on & (1 << s) {
                    0 => Ink::LedOff(kind),
                    _ => Ink::Led { kind, f: ((s + 1) * 255 / n) as u8 },
                };
                let y = (top + h) as i32 - ((s + 1) * SEG_PITCH) as i32 + 1;
                op(Op::Rect { x, y, w: bar_w, h: SEG_PX, fill: true, t: 0, ink });
            }
        }
    }
}

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

    /// The mean of the newest value of each bucket in the `window_ms` that ends at the newest
    /// sample, for the big number. Empty buckets do not count, so a gap does not pull it to zero.
    pub fn mean(&self, window_ms: u64) -> Option<f32> {
        let end = self.last_t? / BUCKET_MS;
        let start = end.saturating_sub((window_ms / BUCKET_MS).max(1) - 1);
        let (sum, n) = (start..=end)
            .filter_map(|span| self.bucket(span))
            .fold((0.0, 0u32), |(sum, n), b| (sum + b.last, n + 1));
        (n > 0).then(|| sum / n as f32)
    }
}

/// The history of a CPU or GPU row.
pub type Fast = Series<BUCKETS>;
/// The history of the memory row: 5 minutes.
pub type Slow = Series<MEM_BUCKETS>;

/// The three rows.
pub struct Model {
    pub cpu: Fast,
    pub bars: Bars,
    pub gpu: Fast,
    pub mem: Slow,
}

impl Model {
    pub const fn new() -> Self {
        Self { cpu: Fast::new(), bars: Bars::new(), gpu: Fast::new(), mem: Slow::new() }
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
        self.bars.take(r);
        if let (Some(v), Some(t)) = (r.gpu, r.tg) {
            self.gpu.add(t, v, v, v);
        }
        // Memory goes on the chart in GB: its y axis zooms to the 5 minutes (`Scale`).
        if let (Some(gb), Some(t)) = (r.mem, r.tm) {
            let (lo, hi) = (r.mem_lo.unwrap_or(gb), r.mem_hi.unwrap_or(gb));
            self.mem.add(t, gb, lo, hi);
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
    op(Op::Rect { x: left as i32, y: top as i32, w, h, fill: true, t: 0, ink: Ink::Background });
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
        op(Op::Line { x0: x, y0: top as i32, x1: x, y1: top as i32 + 4, ink: Ink::Grid(row.index) });
        let Some(earlier) = tick.checked_sub(tick_ms) else { break };
        tick = earlier;
    }

    // Newest first, so each bucket can join the one to its right.
    let mut curve = Curve::new(left, w);
    let now_span = now / BUCKET_MS;
    let mut newer: Option<(u64, i32, i32)> = None; // span, x, y of its newest value
    for back in 0..BUCKETS as u64 {
        let Some(span) = now_span.checked_sub(back) else { break };
        let Some(b) = series.bucket(span) else { continue };
        let x = x_at(span * BUCKET_MS + BUCKET_MS / 2).min(right);
        if x < left {
            break;
        }
        curve.stroke(x, y_at(b.hi), y_at(b.lo));
        let y = y_at(b.last);
        if let Some((n_span, n_x, n_y)) = newer {
            if (n_span - span) * BUCKET_MS <= GAP_MS {
                curve.join(x, y, n_x, n_y);
            }
        }
        newer = Some((span, x, y));
    }
    curve.draw(Ink::Line(row.index), &mut op);
}

/// A curve made of vertical strokes and the lines that join them, kept as the rows it covers in
/// each plot column and drawn with as few lines as cover the same pixels.
///
/// Every part of the curve in a column touches that column's join point, so the rows it covers
/// there are one unbroken span: one vertical line. Next columns of one pixel each, level or one
/// row up or down at each step, are one horizontal or diagonal line. Drawn as a stroke and a join
/// for each bucket, a full chart was about 740 lines on the RA8875, most of them 1 or 2 px.
struct Curve {
    left: i32,
    w: usize,
    /// Top and bottom row in each column; top > bottom where the column is empty.
    spans: [(i32, i32); MAX_PLOT_W],
}

impl Curve {
    fn new(left: i32, w: u16) -> Self {
        Self { left, w: (w as usize).min(MAX_PLOT_W), spans: [(i32::MAX, i32::MIN); MAX_PLOT_W] }
    }

    fn cover(&mut self, x: i32, y: i32) {
        if let Some(s) = self.spans[..self.w].get_mut((x - self.left) as usize) {
            *s = (s.0.min(y), s.1.max(y));
        }
    }

    fn stroke(&mut self, x: i32, y0: i32, y1: i32) {
        self.cover(x, y0);
        self.cover(x, y1);
    }

    /// The pixels of a line, as Framebuf::line puts them.
    fn join(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        let (mut x, mut y) = (x0, y0);
        let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
        let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
        let mut err = dx + dy;
        loop {
            self.cover(x, y);
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

    fn draw(&self, ink: Ink, op: &mut impl FnMut(Op)) {
        let one = |i: usize| self.spans.get(i).filter(|s| s.0 == s.1).map(|s| s.0);
        let mut i = 0;
        while i < self.w {
            let (top, bottom) = self.spans[i];
            let x = self.left + i as i32;
            if top > bottom {
                i += 1;
                continue;
            }
            if top < bottom {
                op(Op::Line { x0: x, y0: top, x1: x, y1: bottom, ink });
                i += 1;
                continue;
            }
            // A run of one-pixel columns with the same step, 0 or 1 row, at each column.
            let mut end = i;
            let mut step = None;
            while end + 1 < self.w {
                let Some(next) = one(end + 1) else { break };
                let d = next - self.spans[end].0;
                if d.abs() > 1 || step.is_some_and(|s| s != d) {
                    break;
                }
                step = Some(d);
                end += 1;
            }
            let (x1, y1) = (self.left + end as i32, self.spans[end].0);
            op(Op::Line { x0: x, y0: top, x1, y1, ink });
            i = end + 1;
        }
    }
}

/// The memory row's compressed time axis (docs/device-v2.md, "Settled parameters"). Column `x` counts from the right edge,
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

/// The memory scale steps, in GB. The scale is rounded out to the first step that
/// gives 5 steps or less, so it does not move on every frame.
const MEM_STEPS_GB: [f32; 10] = [0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0];
/// The smallest span of the memory scale, so that noise does not look large.
pub const MEM_MIN_SPAN_GB: f32 = 1.0;

/// The memory row's y scale: the lowest and the highest GB in the 5 minutes, rounded out to steps
/// (v1 `nice_range`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scale {
    pub lo: f32,
    pub hi: f32,
}

impl Scale {
    pub fn nice(mut lo: f32, mut hi: f32) -> Self {
        if hi - lo < MEM_MIN_SPAN_GB {
            let mid = (lo + hi) / 2.0;
            (lo, hi) = (mid - MEM_MIN_SPAN_GB / 2.0, mid + MEM_MIN_SPAN_GB / 2.0);
        }
        let step = MEM_STEPS_GB.into_iter().find(|&s| (hi - lo) / s <= 5.0).unwrap_or(100.0);
        // The small bias stops float noise from adding a step: 7.0 / 0.1 is 70.00001.
        let lo = (libm::floorf(lo / step + 1e-4) * step).max(0.0);
        let hi = libm::ceilf(hi / step - 1e-4) * step;
        Self { lo, hi: hi.max(lo + step) }
    }

    /// Plot row for `gb`: `lo` on the bottom row of the plot area, `hi` on the top.
    fn y(&self, row: &Row, gb: f32) -> i32 {
        row.plot_y(100.0 * (gb - self.lo) / (self.hi - self.lo)) as i32
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
///
/// The y axis zooms to the 5 minutes (`Scale`). Returns the scale, for the limit labels, or None
/// when there is no memory in the window.
pub fn draw_compressed(
    series: &Slow,
    row: &Row,
    axis: &Axis,
    now: u64,
    mut op: impl FnMut(Op),
) -> Option<Scale> {
    blank(row, &mut op);
    let (left, top, w, _) = row.plot();
    let right = left as i32 + w as i32 - 1;

    for age in MEM_MARKS_MS {
        if let Some(x) = axis.column(age) {
            let x = right - x as i32;
            op(Op::Line { x0: x, y0: top as i32, x1: x, y1: top as i32 + 4, ink: Ink::Grid(row.index) });
        }
    }

    // Merge the buckets into columns, newest first. The scale needs every column before the first
    // stroke. This walks up to 3600 buckets on each frame, so it keeps a ring index and a column
    // cursor: no 64-bit division and no search per bucket.
    let empty = Column { x: 0, lo: 0.0, hi: 0.0, last: 0.0, newest_t: 0, oldest_t: 0 };
    let mut cols = [empty; MAX_PLOT_W];
    let mut n = 0;
    let now_span = now / BUCKET_MS;
    let mut slot = (now_span % MEM_BUCKETS as u64) as usize;
    let into_span = (now - now_span * BUCKET_MS) as u32;
    let mut x = 0;
    for back in 0..MEM_BUCKETS as u64 {
        let Some(span) = now_span.checked_sub(back) else { break };
        let b = series.buckets[slot];
        slot = if slot == 0 { MEM_BUCKETS - 1 } else { slot - 1 };
        if b.span != span {
            continue;
        }
        let t = span * BUCKET_MS + BUCKET_MS / 2;
        // now - t, in 32 bits: the 64-bit product is software on the M33, and this runs 3600
        // times a frame.
        let age = (back as u32 * BUCKET_MS as u32 + into_span).saturating_sub(BUCKET_MS as u32 / 2);
        while x < axis.w && age >= axis.edges[x + 1] {
            x += 1;
        }
        if x == axis.w {
            break;
        }
        match cols[..n].last_mut() {
            Some(c) if c.x == x => {
                c.lo = c.lo.min(b.lo);
                c.hi = c.hi.max(b.hi);
                c.oldest_t = t;
            }
            _ => {
                cols[n] = Column { x, lo: b.lo, hi: b.hi, last: b.last, newest_t: t, oldest_t: t };
                n += 1;
            }
        }
    }
    let cols = &cols[..n];
    let lo = cols.iter().map(|c| c.lo).reduce(f32::min)?;
    let hi = cols.iter().map(|c| c.hi).reduce(f32::max)?;
    let scale = Scale::nice(lo, hi);
    let y_at = |v: f32| scale.y(row, v);

    // The area under the curve first, so the strokes go on top of it. Next columns with one top
    // share one rectangle: the memory curve is mostly flat, and each rectangle is one engine run.
    let bottom = (top + row.plot().3) as i32;
    // A column joined to the next newer one shades up to it, so the columns with no bucket under
    // a join line are not left dark.
    let mut run: Option<(i32, i32, i32)> = None; // left x, right x, top y
    for (i, c) in cols.iter().enumerate().rev() {
        let (x, y) = (right - c.x as i32, y_at(c.lo) + 1);
        let end = match i.checked_sub(1).map(|j| cols[j]) {
            Some(n) if joined(c, &n) => right - n.x as i32 - 1,
            _ => x,
        };
        run = match run {
            Some((l, r, ry)) if ry == y && x == r + 1 => Some((l, end, y)),
            _ => {
                if let Some(r) = run {
                    fill(r, bottom, row, &mut op);
                }
                Some((x, end, y))
            }
        };
    }
    if let Some(r) = run {
        fill(r, bottom, row, &mut op);
    }

    let mut curve = Curve::new(left as i32, row.plot().2);
    for (i, c) in cols.iter().enumerate() {
        let x = right - c.x as i32;
        curve.stroke(x, y_at(c.hi), y_at(c.lo));
        if let Some(n) = i.checked_sub(1).map(|j| cols[j]) {
            if joined(c, &n) {
                curve.join(x, y_at(c.last), right - n.x as i32, y_at(n.last));
            }
        }
    }
    curve.draw(Ink::Line(row.index), &mut op);
    Some(scale)
}

/// True if column `c` and the next newer column `n` are one stretch of curve: next to each other,
/// or with no more than `GAP_MS` between their data.
fn joined(c: &Column, n: &Column) -> bool {
    c.x - n.x == 1 || n.oldest_t - c.newest_t <= GAP_MS
}

/// One run of the shaded area: columns `l` to `r`, from row `y` down to `bottom` (exclusive).
fn fill((l, r, y): (i32, i32, i32), bottom: i32, row: &Row, op: &mut impl FnMut(Op)) {
    if y < bottom {
        let (w, h) = ((r - l + 1) as u16, (bottom - y) as u16);
        op(Op::Rect { x: l, y, w, h, fill: true, t: 0, ink: Ink::Fill(row.index) });
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
        assert!(matches!(out[0], Op::Rect { fill: true, ink: Ink::Background, .. }), "draw starts by clearing");
        out.remove(0);
        out
    }

    /// The pixels of a line, as the panel draws it: Bresenham, both ends included.
    fn line_pixels(x0: i32, y0: i32, x1: i32, y1: i32) -> Vec<(i32, i32)> {
        let mut out = Vec::new();
        let (mut x, mut y) = (x0, y0);
        let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
        let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
        let mut err = dx + dy;
        loop {
            out.push((x, y));
            if x == x1 && y == y1 {
                return out;
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

    /// The curve pixels of `ops` in each column, left to right: (x, top y, bottom y).
    fn columns(ops: &[Op]) -> Vec<(i32, i32, i32)> {
        let mut px: Vec<(i32, i32)> = ops
            .iter()
            .filter_map(|op| match *op {
                Op::Line { x0, y0, x1, y1, ink: Ink::Line(_) } => Some(line_pixels(x0, y0, x1, y1)),
                _ => None,
            })
            .flatten()
            .collect();
        px.sort();
        let mut out: Vec<(i32, i32, i32)> = Vec::new();
        for (x, y) in px {
            match out.last_mut() {
                Some(c) if c.0 == x => c.2 = y,
                _ => out.push((x, y, y)),
            }
        }
        out
    }

    fn reply(t: u64, cores: &[u8]) -> Reply {
        let hex: String = cores.iter().map(|c| format!("{c:02x}")).collect();
        crate::wire::parse(format!(r#"{{"tc":{t},"cores":"{hex}","nsuper":1}}"#).as_bytes()).unwrap()
    }

    #[test]
    fn a_bar_follows_its_core_with_a_short_ema() {
        let mut bars = Bars::new();
        bars.take(&reply(1000, &[0, 0]));
        bars.take(&reply(1100, &[100, 0]));
        // One time constant: 1 - 1/e of the step.
        let v = bars.level(0);
        assert!((v - 63.2).abs() < 0.5, "{v}");
        bars.take(&reply(2000, &[100, 0]));
        assert!(bars.level(0) > 99.0);
        assert_eq!((bars.len(), bars.supers()), (2, 1));
    }

    #[test]
    fn the_peak_takes_the_raw_value_holds_then_falls() {
        let mut bars = Bars::new();
        bars.take(&reply(1000, &[80]));
        bars.take(&reply(1033, &[0]));
        assert_eq!(bars.peak(0, 1033), 80.0, "the spike, not the smoothed level");
        bars.take(&reply(1500, &[0]));
        assert_eq!(bars.peak(0, 1000 + HOLD_MS), 80.0, "held");
        assert_eq!(bars.peak(0, 1000 + HOLD_MS + 1000), 80.0 - FALL_PER_S);
        assert!((bars.peak(0, 10_000) - bars.level(0)).abs() < 1e-3, "it stops at the bar");
    }

    #[test]
    fn a_new_spike_under_a_falling_peak_restarts_the_hold() {
        let mut bars = Bars::new();
        bars.take(&reply(0, &[90]));
        bars.take(&reply(2000, &[0]));
        assert_eq!(bars.peak(0, 2000), 90.0 - 42.0, "1.4 s of fall at 30 %/s");
        bars.take(&reply(2100, &[70]));
        assert_eq!(bars.peak(0, 2100 + HOLD_MS), 70.0);
    }

    #[test]
    fn the_bars_reset_when_the_core_count_changes() {
        let mut bars = Bars::new();
        bars.take(&reply(0, &[90, 90]));
        bars.take(&reply(33, &[10, 10, 10]));
        assert_eq!(bars.len(), 3);
        assert_eq!(bars.peak(0, 33), 10.0);
    }

    /// The segment rects of one draw, as (x, y, w, ink), in the order drawn.
    fn segments(view: &mut BarsView, bars: &Bars, now: u64) -> Vec<(i32, i32, u16, Ink)> {
        let mut out = Vec::new();
        view.draw(bars, &row(), now, |op| match op {
            Op::Rect { x, y, w, h: SEG_PX, ink, .. } => out.push((x, y, w, ink)),
            Op::Rect { ink: Ink::Background, .. } => {}
            op => panic!("not a segment: {op:?}"),
        });
        out
    }

    #[test]
    fn each_bar_is_a_stack_of_segments_lit_to_its_level_with_one_peak_segment() {
        let mut bars = Bars::new();
        bars.take(&reply(0, &[100, 100]));
        bars.take(&reply(500, &[60, 0]));
        let n = segs(&row());
        let ops = segments(&mut BarsView::new(), &bars, 500);
        assert_eq!(ops.len(), 2 * n, "every segment of both bars");
        let lit = |kind: u8| ops.iter().filter(move |o| matches!(o.3, Ink::Led { kind: k, .. } if k == kind)).count();
        assert_eq!(lit(0), (n as f32 * 0.6).round() as usize + 1, "super bar: 60 % lit, plus the peak at the top");
        assert_eq!(lit(1), 1, "performance bar: only its peak");
        let top = ops.iter().filter(|o| o.0 == ops[0].0).min_by_key(|o| o.1).unwrap();
        assert!(matches!(top.3, Ink::Led { kind: 0, f: 255 }), "the peak at 100 % is the top segment, at full red");
        let (left, top_y, w, h) = row().plot();
        assert_eq!(ops.iter().map(|o| o.1).min(), Some(top_y as i32 + h as i32 - (n * SEG_PITCH) as i32 + 1));
        assert_eq!(ops.iter().map(|o| o.0).min(), Some(left as i32), "no padding on the left");
        assert_eq!(ops.iter().map(|o| o.0 + o.2 as i32).max(), Some((left + w) as i32 - 1), "or the right, but the gap");
    }

    #[test]
    fn a_frame_redraws_only_the_segments_that_changed() {
        let mut bars = Bars::new();
        bars.take(&reply(0, &[0, 0]));
        let mut view = BarsView::new();
        segments(&mut view, &bars, 0);
        assert_eq!(segments(&mut view, &bars, 33), vec![], "nothing moved");
        bars.take(&reply(5000, &[0, 100]));
        let changed = segments(&mut view, &bars, 5000);
        assert_eq!(changed.len(), segs(&row()), "the second bar fills; the first stays dark");
        view.forget();
        assert_eq!(segments(&mut view, &bars, 5000).len(), 2 * segs(&row()), "after a blank, all again");
        bars.take(&reply(5033, &[0, 0, 0]));
        assert_eq!(segments(&mut view, &bars, 5033).len(), 3 * segs(&row()), "a new core count moves every bar");
    }

    #[test]
    fn the_merged_curve_lights_the_same_pixels_as_its_strokes_and_joins() {
        let (left, w) = (10, 60);
        let mut seed = 7u32;
        let mut rand = |n: i32| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            (seed >> 16) as i32 % n
        };
        for _ in 0..200 {
            let mut curve = Curve::new(left, w);
            let mut want = Vec::new();
            let mut newer: Option<(i32, i32)> = None;
            let mut x = left + w as i32 - 1;
            while x >= left {
                let y = 20 + rand(5) - 2 + newer.map_or(0, |n| n.1 - 20);
                let (hi, lo) = (y - rand(2), y + rand(2));
                curve.stroke(x, hi, lo);
                want.extend(line_pixels(x, hi, x, lo));
                if let Some((n_x, n_y)) = newer.filter(|_| rand(8) != 0) {
                    curve.join(x, y, n_x, n_y);
                    want.extend(line_pixels(x, y, n_x, n_y));
                }
                newer = Some((x, y));
                x -= 1 + rand(3);
            }
            let mut ops = Vec::new();
            curve.draw(Ink::Line(0), &mut |op| ops.push(op));
            want.sort();
            want.dedup();
            let mut got: Vec<_> = columns(&ops).iter().flat_map(|&(x, t, b)| (t..=b).map(move |y| (x, y))).collect();
            got.sort();
            assert_eq!(got, want);
            // Each drawn pixel once: no line overlaps another.
            let drawn: usize = ops.iter().map(|op| match *op {
                Op::Line { x0, y0, x1, y1, .. } => line_pixels(x0, y0, x1, y1).len(),
                _ => 0,
            }).sum();
            assert_eq!(drawn, want.len());
        }
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
    fn the_mean_covers_the_window_that_ends_at_the_newest_sample() {
        let mut s = Fast::new();
        assert_eq!(s.mean(1000), None);
        // One sample per bucket, 0 to 19. A 1 s window is 12 buckets: the newest 12 are 8 to 19.
        for i in 0..20u64 {
            s.add(100 * BUCKET_MS + i * BUCKET_MS, i as f32, i as f32, i as f32);
        }
        assert_eq!(s.mean(1000), Some((8..20).sum::<u64>() as f32 / 12.0));
        assert_eq!(s.mean(BUCKET_MS), Some(19.0));
    }

    #[test]
    fn the_mean_skips_empty_buckets() {
        let mut s = Fast::new();
        s.add(100 * BUCKET_MS, 10.0, 10.0, 10.0);
        s.add(105 * BUCKET_MS, 30.0, 30.0, 30.0);
        assert_eq!(s.mean(1000), Some(20.0));
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
        let xs: Vec<i32> = columns(&ops(&s, now)).iter().map(|c| c.0).collect();
        // 12 px per second, within the rounding of a span to its centre, joined by a level line.
        assert!((xs[xs.len() - 1] - right).abs() <= 1, "{xs:?}");
        assert!((xs[0] - (right - 12)).abs() <= 1, "{xs:?}");
        assert!(xs.windows(2).all(|p| p[1] == p[0] + 1), "{xs:?}");
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
        // Ticks at 10 s, 5 s and 0 s: 2.5, 7.5 and 12.5 s ago, at 83 ms per pixel 30.1, 90.4 and
        // 150.6 px.
        let now = 12_500;
        let ticks: Vec<i32> = ops(&s, now)
            .iter()
            .map(|op| match *op {
                Op::Line { x0, .. } => x0,
                _ => panic!(),
            })
            .collect();
        assert_eq!(ticks, vec![right - 30, right - 90, right - 151]);
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
    fn the_model_puts_memory_on_the_chart_in_gb() {
        let mut m = Model::new();
        let r = crate::wire::parse(
            br#"{"pull":1,"v":2,"t":1000,"cpu":12.5,"tc":999,"mem":32.0,"mem_lo":16.0,"mem_hi":64.0,"tm":990,"mem_total":128.0,"ok":false}"#,
        )
        .unwrap();
        m.take(&r);
        assert_eq!(m.cpu.newest(), Some(12.5));
        assert_eq!(m.gpu.newest(), None);
        let b = m.mem.bucket(990 / BUCKET_MS).unwrap();
        assert_eq!((b.lo, b.hi, b.last), (16.0, 64.0, 32.0));
    }

    fn mem_row() -> Row {
        layout::rows(400, 240)[MEM]
    }

    /// The curve pixels `draw_compressed` lights in each column: (x, top y, bottom y).
    fn mem_strokes(series: &Slow, now: u64) -> Vec<(i32, i32, i32)> {
        let row = mem_row();
        let axis = Axis::new(row.plot().2);
        let mut out = Vec::new();
        draw_compressed(series, &row, &axis, now, |op| out.push(op));
        columns(&out)
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
        assert!((xs[xs.len() - 1] - right).abs() <= 1, "{xs:?}");
        assert!((xs[0] - (right - 12)).abs() <= 1, "{xs:?}");
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
        // 10 Hz at 20 GB, with one 100 ms sample at 90 GB three minutes ago.
        let start = now - MEM_WINDOW_MS + 1000;
        for i in 0..(MEM_WINDOW_MS - 2000) / 100 {
            let t = start + i * 100;
            let v = if t == now - 180_000 { 90.0 } else { 20.0 };
            s.add(t, v, v, v);
        }
        let peak_y = Scale::nice(20.0, 90.0).y(&mem_row(), 90.0);
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

    #[test]
    fn the_memory_scale_rounds_out_to_steps_as_in_v1() {
        // The cases of v1's test_nice_range (tests/test_mem_chart.py). One differs: v1 gives 7.4
        // for 8.0..8.2, because float noise makes floor(7.6 / 0.2) 37 there. 7.6 is the step.
        let cases = [
            ((7.3, 9.1), (7.0, 9.5)),
            ((8.0, 8.2), (7.6, 8.6)),
            ((0.0, 0.1), (0.0, 0.6)),
            ((12.4, 61.9), (10.0, 70.0)),
            ((100.0, 127.5), (100.0, 130.0)),
            ((-2.0, 0.3), (0.0, 0.5)),
        ];
        for ((lo, hi), (want_lo, want_hi)) in cases {
            let s = Scale::nice(lo, hi);
            assert!((s.lo - want_lo).abs() < 1e-3 && (s.hi - want_hi).abs() < 1e-3, "{lo}..{hi}: {s:?}");
        }
    }

    #[test]
    fn the_memory_scale_holds_still_while_memory_moves_a_little() {
        assert_eq!(Scale::nice(61.2, 63.9), Scale::nice(61.4, 63.6));
    }

    #[test]
    fn the_memory_row_zooms_to_its_five_minutes() {
        let mut s = Slow::new();
        let now = 1_000_000;
        // 61 to 63 GB: on a 0-128 GB axis this is a flat line 1 px high.
        for i in 0..1000 {
            let v = 61.0 + (i % 21) as f32 / 10.0;
            s.add(now - 100_000 + i * 100, v, v, v);
        }
        let row = mem_row();
        let axis = Axis::new(row.plot().2);
        let scale = draw_compressed(&s, &row, &axis, now, |_| {});
        assert_eq!(scale, Some(Scale { lo: 61.0, hi: 63.0 }));
        let ys: Vec<i32> = mem_strokes(&s, now).iter().flat_map(|&(_, a, b)| [a, b]).collect();
        let span = ys.iter().max().unwrap() - ys.iter().min().unwrap();
        assert!(span as u16 > row.plot().3 / 2, "the curve fills {span} px");
    }

    #[test]
    fn an_empty_memory_row_has_no_scale() {
        let row = mem_row();
        let axis = Axis::new(row.plot().2);
        assert_eq!(draw_compressed(&Slow::new(), &row, &axis, 1_000_000, |_| {}), None);
    }
}
