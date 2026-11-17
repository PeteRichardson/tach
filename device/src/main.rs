//! Tach device firmware, milestone 1: bring up the SHARP scaffold, prove the geometry, and chart
//! the Mac. Each frame redraws the curves from the chart model at Mac time, then pulls one reply
//! from tach-sender.py and reads it into the model (ADR-0002, ADR-0006).
//!
//! Wiring: device/WIRING.md. Pins: SPI0 SCK GP18, MOSI GP19, MISO GP16, CS GP17 (GPIO, active
//! low), RST GP15. GP20/GP21 stay free for SWD.

#![no_std]
#![no_main]

use panic_halt as _;

use core::fmt::Write as _;

use embedded_hal::spi::MODE_0;
use fugit::RateExtU32;
use rp235x_hal::{
    self as hal,
    clocks::init_clocks_and_plls,
    Clock,
    gpio::{FunctionSpi, Pins},
    sio::Sio,
    spi::Spi,
    watchdog::Watchdog,
};

mod chart;
mod fifo_spi;
mod layout;
mod link;
mod ops;
mod ra8875;
mod text;
mod wire;

use fifo_spi::FifoSpi;
use ops::{Ink, Op, Panel};

#[cfg(feature = "rtt")]
use rtt_target::rprintln;
use ra8875::Ra8875;

/// Board crystal on a Pico 2.
const XOSC_HZ: u32 = 12_000_000;
/// SPI clock until the RA8875 PLL runs. It is off at reset, so the bus must be slow.
const SPI_INIT_HZ: u32 = 1_000_000;
/// SPI clock after init: 150 MHz / 8. At the next step up, 25 MHz, 28% of writes did not arrive
/// (measured 2026-11-17 with the bring-up link test, 0 errors at this clock). Reads do not work
/// above about 6 MHz: every byte reads back one bit late (measured 2026-11-16). So after init the
/// driver reads nothing and waits on the WAIT pin.
const SPI_HZ: u32 = 18_750_000;
/// Row labels, in the order of chart::CPU, GPU, MEM.
const LABELS: [&[u8]; layout::ROWS] = [b"CPU", b"GPU", b"MEM"];
/// Target frames per second.
const FPS: u32 = 30;
/// The least time to wait for a reply after a pull, also when the frame has run past its
/// deadline. Replies took at most 1.6 ms in the logs. Without this, a frame longer than 1 / FPS
/// gave up on its reply at once, and the curves had gaps (seen 2026-11-17).
const REPLY_WAIT_US: u64 = 5_000;
/// The CPU and GPU numbers show the mean of this much history. One sample alone jumps by 10 to 15
/// points from one frame to the next, which is too fast to read.
const NUMBER_MEAN_MS: u64 = 1000;
/// The CPU and GPU numbers change only this often. Memory changes slowly and updates every frame.
const NUMBER_REFRESH_US: u64 = 300_000;

/// The RP2350 timer runs at 1 MHz, so ticks are microseconds.
const TICKS_PER_S: u64 = 1_000_000;

/// Busy-wait on the 1 MHz timer. Milestone 1 has no async runtime, and a busy delay is the honest
/// way to measure what the panel costs without a scheduler in the way.
fn wait_us(timer: &hal::timer::Timer<hal::timer::CopyableTimer0>, us: u64) {
    let end = timer.get_counter().ticks() + us;
    while timer.get_counter().ticks() < end {
        core::hint::spin_loop();
    }
}

/// Tell the RP2350 boot ROM that this is a secure Arm executable. Without this block in the
/// first 4 KiB of flash the ROM does not start the image and drops back to BOOTSEL.
#[link_section = ".start_block"]
#[used]
pub static IMAGE_DEF: hal::block::ImageDef = hal::block::ImageDef::secure_exe();

#[hal::entry]
fn main() -> ! {
    #[cfg(feature = "rtt")]
    rtt_target::rtt_init_print!();

    let Some(pac) = hal::pac::Peripherals::take() else {
        hal::halt()
    };

    let mut resets = pac.RESETS;
    let mut watchdog = Watchdog::new(pac.WATCHDOG);
    let Ok(clocks) = init_clocks_and_plls(
        XOSC_HZ,
        pac.XOSC,
        pac.CLOCKS,
        pac.PLL_SYS,
        pac.PLL_USB,
        &mut resets,
        &mut watchdog,
    ) else {
        hal::halt()
    };

    let sio = Sio::new(pac.SIO);
    let pins = Pins::new(pac.IO_BANK0, pac.PADS_BANK0, sio.gpio_bank0, &mut resets);

    // CS before the SPI pins, so the chip stays deselected. The 10k pull-up holds it until here.
    let cs = pins.gpio17.into_push_pull_output();
    let mut rst = pins.gpio15.into_push_pull_output();
    let wait = pins.gpio13.into_pull_up_input();
    let sck = pins.gpio18.into_function::<FunctionSpi>();
    let mosi = pins.gpio19.into_function::<FunctionSpi>();
    let miso = pins.gpio16.into_function::<FunctionSpi>();
    let spi = Spi::<_, _, _, 8>::new(pac.SPI0, (mosi, miso, sck)).init(
        &mut resets,
        clocks.peripheral_clock.freq(),
        SPI_INIT_HZ.Hz(),
        MODE_0,
    );

    let timer = hal::timer::Timer::new_timer0(pac.TIMER0, &mut resets, &clocks);

    // USB first, so the host can enumerate the port while the panel comes up.
    link::init(pac.USB, pac.USB_DPRAM, clocks.usb_clock, &mut resets);

    let mut bus = ra8875::Bus::new(spi, cs);
    if let Err(id) = ra8875::init(&mut bus, &mut rst, |us| wait_us(&timer, us)) {
        rprintln!("RA8875 not found: ID {:#04x}, want {:#04x}. See device/WIRING.md.", id, ra8875::CHIP_ID);
        hal::halt()
    }

    // The PLL runs now, so the bus can go to full speed. The divider cannot make every rate, so
    // log the one the bus really runs at.
    let (mut spi, cs) = bus.free();
    let spi_hz = spi.set_baudrate(clocks.peripheral_clock.freq(), SPI_HZ.Hz()).to_Hz();

    // From here, the SPI port drives CS itself, and each access is one 16-bit frame. See
    // fifo_spi.rs.
    let mut panel = Ra8875::new(FifoSpi::new(spi, cs.into_function::<FunctionSpi>()), wait);

    let (w, h) = panel.size();
    rprintln!("tach up: RA8875 {}x{} @ {} Hz SPI ({} Hz requested)", w, h, spi_hz, SPI_HZ);

    selftest(&mut panel, &timer);

    let rows = layout::rows(w, h);
    // The frame loop never redraws the chrome: present it once with the whole panel, then copy
    // only the plots and number slots.
    draw_chrome(&mut panel, &rows);
    panel.present();

    // --- frame loop ----------------------------------------------------------
    // Static chrome once, then per-frame work only. This is the shape the firmware keeps on the
    // RA8875, so the numbers measured here carry over to the target panel.
    let frame_us = TICKS_PER_S / FPS as u64;
    let mut model = chart::Model::new();
    let mem_axis = chart::Axis::new(rows[chart::MEM].plot().2);
    // Mac monotonic ms minus device ms, from the newest reply, and the device time of that reply.
    // Both None until the first reply. chart::mac_now turns them into the Mac time of a frame.
    let mut mac_offset_ms: Option<i64> = None;
    let mut last_reply_us: Option<u64> = None;

    let mut frames: u32 = 0;
    let mut draw_us_max: u64 = 0;
    let mut present_us_max: u64 = 0;
    let mut bytes_max: usize = 0;
    let mut ops_max: usize = 0;
    let mut bytes_total: usize = 0;
    let mut window_start: u64 = timer.get_counter().ticks();

    // One pull after each present, then wait for the reply in the idle part of the frame.
    let mut pull_seq: u32 = 0;
    let mut reply = [0u8; link::LINE_MAX];
    let mut rtt_us_max: u64 = 0;
    let mut rtt_us_sum: u64 = 0;
    let mut replies: u32 = 0;
    let mut misses: u32 = 0; // pull sent, no matching reply before the frame deadline
    let mut unsent: u32 = 0; // port not open, or no room in the USB buffer
    let mut unread: u32 = 0; // matching reply, but not a version 2 object

    // The CPU and GPU numbers on the glass, held between refreshes.
    let mut fast_numbers: [Option<text::Number>; 2] = [None, None];
    let mut next_number_us: u64 = 0;
    // What each number slot shows on layer 2: its number, ink and memory scale. A slot is drawn
    // and copied only when this changes. Text was about 4 ms of each frame.
    let mut shown: [Option<(text::Number, Ink, Option<chart::Scale>)>; layout::ROWS] = [None; layout::ROWS];

    loop {
        let t0 = timer.get_counter().ticks();

        // Every curve moves on every frame, whether or not a sample came in: travel follows the
        // clock (ADR-0002). The reply read at the end of this frame shows on the next one. With
        // no Mac time (no reply yet, or none for STALE_MS) the plots are blank, and every row is
        // stale. A stale row shows "--" in its number slot.
        if t0 >= next_number_us {
            fast_numbers = [
                model.cpu.mean(NUMBER_MEAN_MS).map(text::Number::percent),
                model.gpu.mean(NUMBER_MEAN_MS).map(text::Number::percent),
            ];
            next_number_us = t0 + NUMBER_REFRESH_US;
        }
        let [cpu_number, gpu_number] = &fast_numbers;
        let numbers = [cpu_number, gpu_number, &model.mem.newest().map(text::Number::gb)];
        let now = chart::mac_now(mac_offset_ms, last_reply_us, t0);
        let mut stale = [true; layout::ROWS];
        let mut windows = [(0, 0, 0, 0); 2 * layout::ROWS];
        let mut copy_n = 0;
        for (i, (row, stale)) in rows.iter().zip(stale.iter_mut()).enumerate() {
            // The memory scale, for its limit labels: None until there is memory to zoom to.
            let mut scale = None;
            match now {
                Some(now) => {
                    match i {
                        chart::CPU => chart::draw(&model.cpu, row, now, |op| panel.apply(op)),
                        chart::GPU => chart::draw(&model.gpu, row, now, |op| panel.apply(op)),
                        _ => {
                            scale = chart::draw_compressed(&model.mem, row, &mem_axis, now, |op| {
                                panel.apply(op)
                            })
                        }
                    }
                    *stale = model.is_stale(i, now);
                }
                None => chart::blank(row, |op| panel.apply(op)),
            }
            let (number, ink) = match &numbers[i] {
                Some(n) if !*stale => (n, Ink::Line(row.index)),
                _ => (&text::Number::dashes(), Ink::Dim),
            };
            windows[copy_n] = row.plot();
            copy_n += 1;
            let slot = Some((*number, ink, scale));
            if shown[i] == slot {
                continue;
            }
            shown[i] = slot;
            windows[copy_n] = row.slot();
            copy_n += 1;
            if i == chart::MEM {
                let limits = scale.map(|s| (text::Number::gb(s.lo), text::Number::gb(s.hi)));
                text::draw_mem_slot(row, LABELS[i], number, limits, ink, |op| panel.apply(op));
            } else {
                text::draw_slot(row, LABELS[i], number, ink, |op| panel.apply(op));
            }
        }

        let t1 = timer.get_counter().ticks();
        panel.copy_only(&windows[..copy_n]);
        panel.present();
        let t2 = timer.get_counter().ticks();

        // Pull, then wait for the reply until the frame deadline.
        pull_seq = pull_seq.wrapping_add(1);
        let mut req = LineBuf::new();
        let _ = writeln!(req, "{{\"pull\":{}}}", pull_seq);
        let mut tag = LineBuf::new();
        let _ = write!(tag, "\"pull\":{}", pull_seq);
        if link::send(req.as_bytes()) {
            let deadline = (t0 + frame_us).max(t2 + REPLY_WAIT_US);
            loop {
                let now = timer.get_counter().ticks();
                if let Some(n) = link::take_line(&mut reply) {
                    if answers(&reply[..n], tag.as_bytes()) {
                        rtt_us_max = rtt_us_max.max(now - t2);
                        rtt_us_sum += now - t2;
                        replies += 1;
                        match wire::parse(&reply[..n]).filter(|r| r.v == Some(wire::VERSION)) {
                            Some(r) => {
                                // `t` was stamped on the Mac about half a millisecond ago. At
                                // 12 px/s that is well under a pixel, so it is not corrected.
                                if let Some(t) = r.t {
                                    mac_offset_ms = Some(t as i64 - (now / 1000) as i64);
                                    last_reply_us = Some(now);
                                }
                                model.take(&r);
                            }
                            None => unread += 1,
                        }
                        break;
                    }
                }
                if now >= deadline {
                    misses += 1;
                    break;
                }
            }
        } else {
            unsent += 1;
        }

        draw_us_max = draw_us_max.max(t1 - t0);
        present_us_max = present_us_max.max(t2 - t1);
        bytes_max = bytes_max.max(panel.last_frame_bytes);
        ops_max = ops_max.max(panel.last_frame_ops);
        bytes_total += panel.last_frame_bytes;
        frames += 1;

        // One line a second: achieved rate, worst-case draw and present, and pixel traffic. This is
        // the number the whole design was aimed at — v1 managed 190 to 340 ms per frame.
        let span = t2 - window_start;
        if span >= TICKS_PER_S {
            rprintln!(
                "fps {}  draw_us {}  present_us {}  bus B max {}  ops max {}  KB/s {}  rtt_us avg {} max {}  ok {} miss {} unsent {} unread {}  cpu {:?} gpu {:?} mem {:?}  stale {:?}",
                frames as u64 * TICKS_PER_S / span,
                draw_us_max,
                present_us_max,
                bytes_max,
                ops_max,
                bytes_total as u64 * TICKS_PER_S / span / 1024,
                rtt_us_sum / (replies.max(1) as u64),
                rtt_us_max,
                replies,
                misses,
                unsent,
                unread,
                model.cpu.newest(),
                model.gpu.newest(),
                model.mem.newest(),
                stale
            );
            window_start = t2;
            frames = 0;
            draw_us_max = 0;
            present_us_max = 0;
            bytes_max = 0;
            ops_max = 0;
            bytes_total = 0;
            rtt_us_max = 0;
            rtt_us_sum = 0;
            replies = 0;
            misses = 0;
            unsent = 0;
            unread = 0;
        }

        // Pace to the deadline of this frame, so a slow frame cannot cause a burst of catch-up
        // frames. The pull wait above may already have used some or all of it.
        let spent = timer.get_counter().ticks() - t0;
        if spent < frame_us {
            wait_us(&timer, frame_us - spent);
        }
    }
}

/// A short line built with `write!`, for the pull request and the tag that matches its reply.
struct LineBuf {
    buf: [u8; 32],
    len: usize,
}

impl LineBuf {
    fn new() -> Self {
        Self { buf: [0; 32], len: 0 }
    }

    fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

impl core::fmt::Write for LineBuf {
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

/// True if the reply line carries `tag` (`"pull":N`) as a whole value: a `,` or `}` must come
/// next, so `"pull":12` does not match a reply to pull 123.
fn answers(line: &[u8], tag: &[u8]) -> bool {
    line.windows(tag.len() + 1)
        .any(|w| &w[..tag.len()] == tag && matches!(w[tag.len()], b',' | b'}'))
}

/// Five patterns that answer the geometry questions without needing a font. Each is held half a
/// second. Read them off the glass: if any one is wrong, a shim assumption is wrong.
///
/// 1. all pixels lit      — power, CS polarity, VCOM
/// 2. left column only    — x = 0 and bit order (reversed bit order puts the bar on the right)
/// 3. top row only        — line numbering (off by one shows it on row 239, or nowhere)
/// 4. left half lit       — row stride and byte order
/// 5. rulers plus border  — that 400 x 240 is the real panel size
fn selftest<P: Panel>(panel: &mut P, timer: &hal::timer::Timer<hal::timer::CopyableTimer0>) {
    let (w, h) = panel.size();

    panel.apply(Op::Clear { ink: Ink::Dim });
    panel.present();
    wait_us(timer, 500_000);

    panel.apply(Op::Clear { ink: Ink::Background });
    panel.apply(Op::Line { x0: 0, y0: 0, x1: 0, y1: h as i32 - 1, ink: Ink::Dim });
    panel.present();
    wait_us(timer, 500_000);

    panel.apply(Op::Clear { ink: Ink::Background });
    panel.apply(Op::Line { x0: 0, y0: 0, x1: w as i32 - 1, y1: 0, ink: Ink::Dim });
    panel.present();
    wait_us(timer, 500_000);

    panel.apply(Op::Clear { ink: Ink::Background });
    panel.apply(Op::Rect { x: 0, y: 0, w: w / 2, h, fill: true, t: 0, ink: Ink::Dim });
    panel.present();
    wait_us(timer, 500_000);

    panel.apply(Op::Clear { ink: Ink::Background });
    panel.apply(Op::Rect { x: 0, y: 0, w: w - 1, h: h - 1, fill: false, t: 1, ink: Ink::Dim });
    let mut x: u16 = 0;
    while x < w {
        panel.apply(Op::Line { x0: x as i32, y0: 0, x1: x as i32, y1: h as i32 - 1, ink: Ink::Dim });
        x += 50;
    }
    let mut y: u16 = 0;
    while y < h {
        panel.apply(Op::Line { x0: 0, y0: y as i32, x1: w as i32 - 1, y1: y as i32, ink: Ink::Dim });
        y += 40;
    }
    panel.present();
    wait_us(timer, 1_500_000);
}

/// Static chrome: the panel border and each chart frame. The number slot is drawn per frame by
/// `text::draw_slot`.
fn draw_chrome<P: Panel>(panel: &mut P, rows: &[layout::Row; layout::ROWS]) {
    let (w, h) = panel.size();
    panel.apply(Op::Clear { ink: Ink::Background });
    panel.apply(Op::Rect { x: 0, y: 0, w: w - 1, h: h - 1, fill: false, t: 1, ink: Ink::Dim });
    for r in rows {
        panel.apply(Op::Rect {
            x: r.chart_x as i32,
            y: r.chart_y as i32,
            w: r.chart_w(),
            h: r.chart_h,
            fill: false,
            t: 1,
            ink: Ink::Grid(r.index),
        });
    }
}
