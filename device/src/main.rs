//! Tach device firmware, milestone 1: bring up the SHARP scaffold, prove the geometry, and measure
//! the frame cost and the pull round trip. The pull is a probe for now: the demo curves are still
//! fake, and the reply is only timed, not read.
//!
//! Wiring: device/WIRING.md. Pins: SPI0 SCK GP18, MOSI GP19, CS GP17 (GPIO, active high),
//! DISP GP16 (GPIO). GP20/GP21 stay free for SWD.

#![no_std]
#![no_main]

use panic_halt as _;

use core::fmt::Write as _;

use embedded_hal::spi::MODE_1;
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

mod fifo_spi;
mod framebuf;
mod layout;
mod link;
mod ops;
mod sharp;

use fifo_spi::FifoSpi;
use ops::{Op, Panel};

#[cfg(feature = "rtt")]
use rtt_target::rprintln;
use sharp::Sharp;

/// Board crystal on a Pico 2.
const XOSC_HZ: u32 = 12_000_000;
/// SPI clock request. From a 150 MHz clk_peri the divider gives 10.71 MHz for this, and the next
/// step up is 12.5 MHz. Measured on the glass: 10.71 MHz works, 12.5 MHz leaves the panel blank.
/// See device/BRINGUP.md.
const SPI_HZ: u32 = 12_000_000;
/// Target frames per second for the motion demo.
const FPS: u32 = 30;
/// Fake sample rate for the motion demo, the same 8 Hz the Mac side sends.
const SAMPLE_HZ: u64 = 8;


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
    let mut pins = Pins::new(pac.IO_BANK0, pac.PADS_BANK0, sio.gpio_bank0, &mut resets);

    // SPI0: GP18 SCK, GP19 MOSI. No MISO — the panel takes data only. The HAL pin tuple for a
    // two-wire set is (Tx, Sck); see rp235x_hal::spi::pins.
    let sck = pins.gpio18.into_function::<FunctionSpi>();
    let mosi = pins.gpio19.into_function::<FunctionSpi>();
    let mut spi = Spi::<_, _, _, 8>::new(pac.SPI0, (mosi, sck)).init(
        &mut resets,
        clocks.peripheral_clock.freq(),
        SPI_HZ.Hz(),
        MODE_1,
    );

    // CS is active high, so it is driven as plain GPIO rather than by the SPI peripheral.
    let cs = pins.gpio17.into_push_pull_output();
    let disp = pins.gpio16.into_push_pull_output();

    let timer = hal::timer::Timer::new_timer0(pac.TIMER0, &mut resets, &clocks);

    // USB first, so the host can enumerate the port while the self-test runs.
    link::init(pac.USB, pac.USB_DPRAM, clocks.usb_clock, &mut resets);

    // The divider cannot make every rate, so log the one the bus really runs at: 10.71 MHz for a
    // 12 MHz request. Setting it again writes the same divider.
    let spi_hz = spi.set_baudrate(clocks.peripheral_clock.freq(), SPI_HZ.Hz()).to_Hz();

    // The HAL's own write waits after every byte; this bus keeps the FIFO full. See fifo_spi.rs.
    let mut panel = Sharp::new(FifoSpi::new(spi), cs, disp);

    panel.enable();
    // The driver needs a moment after DISP goes high before it accepts pixels.
    wait_us(&timer, 20_000);

    let (w, h) = panel.size();
    rprintln!("tach scaffold up: {}x{} @ {} Hz SPI ({} Hz requested)", w, h, spi_hz, SPI_HZ);

    selftest(&mut panel, &timer);

    let rows = layout::rows(w, h);
    draw_chrome(&mut panel, &rows);
    panel.present();

    // --- motion demo ---------------------------------------------------------
    // Static chrome once, then per-frame work only. This is the shape the firmware keeps on the
    // RA8875, so the numbers measured here carry over to the target panel.
    let frame_us = TICKS_PER_S / FPS as u64;
    let mut sim: u64 = 0; // fake sample counter at 8 Hz
    let sample_us = TICKS_PER_S / SAMPLE_HZ;
    let mut next_sample: u64 = timer.get_counter().ticks();
    // Pixels the plot has moved since the last sample, so the new sample joins the previous one
    // where it now is: 1 or 2 px back at 12 px/s and 8 Hz.
    let mut px_since_sample: usize = 0;
    let mut phase_x100: u64 = 0; // travel accumulator in hundredths of a pixel
    let travel_x100_per_frame: u64 = (layout::TRAVEL_PX_PER_S * 100.0) as u64 / FPS as u64;

    let mut frames: u32 = 0;
    let mut draw_us_max: u64 = 0;
    let mut present_us_max: u64 = 0;
    let mut present_bytes_at_max: usize = 0;
    let mut bytes_total: usize = 0;
    let mut window_start: u64 = timer.get_counter().ticks();

    // Pull round trip (ADR-0006, open frontier item 2). One pull after each present, then wait
    // for the reply in the idle part of the frame. The demo does not use the reply yet.
    let mut pull_seq: u32 = 0;
    let mut reply = [0u8; link::LINE_MAX];
    let mut rtt_us_max: u64 = 0;
    let mut rtt_us_sum: u64 = 0;
    let mut replies: u32 = 0;
    let mut misses: u32 = 0; // pull sent, no matching reply before the frame deadline
    let mut unsent: u32 = 0; // port not open, or no room in the USB buffer

    loop {
        let t0 = timer.get_counter().ticks();

        // 8 Hz sample stream: CPU/GPU waves, memory a slow rising level. Paced by the timer, not
        // by the travel accumulator, which wraps every pixel.
        let sample_now = t0 >= next_sample;
        if sample_now {
            sim += 1;
            next_sample += sample_us;
        }

        // Travel at 12 px/s: 0.4 px per frame at 30 fps. Accumulate and shift whole pixels when it
        // crosses 1.0 — that uneven 1,0,1,1,0 pattern is exactly what the RA8875's whole-pixel
        // scroll does natively, and why the curve is redrawn at an exact offset instead.
        phase_x100 += travel_x100_per_frame;
        let shift = (phase_x100 / 100) as usize;
        phase_x100 %= 100;
        px_since_sample += shift;
        if shift > 0 {
            for r in rows.iter() {
                let (left, top, width, height) = r.plot();
                panel.apply(Op::ShiftBand { left, top, width, height, by: shift });
            }
        }

        for (i, r) in rows.iter().enumerate() {
            if !sample_now {
                continue;
            }
            let v = match i {
                0 => 30.0 + 25.0 * sine((sim % 200) as f32 / 3.0),
                1 => 15.0 + 10.0 * sine((sim % 137) as f32 / 5.0),
                _ => 40.0 + (sim % 90) as f32 / 3.0,
            }
            .clamp(0.0, 100.0);
            let (plot_x, plot_top, plot_w, _) = r.plot();
            // Newest column of the plot area, just inside the right edge of the frame.
            let now_x = (plot_x + plot_w - 1) as i32;
            let y = r.plot_y(v);
            // Newest sample lands at the right edge; history moves left with time.
            panel.apply(Op::Pix {
                x: now_x,
                y: y as i32,
                on: true,
            });
            // Connect to the previous sample so the curve reads as a line, not dots.
            let prev_v = match i {
                0 => 30.0 + 25.0 * sine((sim.saturating_sub(1) % 200) as f32 / 3.0),
                1 => 15.0 + 10.0 * sine((sim.saturating_sub(1) % 137) as f32 / 5.0),
                _ => 40.0 + (sim.saturating_sub(1) % 90) as f32 / 3.0,
            }
            .clamp(0.0, 100.0);
            let py = r.plot_y(prev_v);
            panel.apply(Op::Line {
                x0: now_x - px_since_sample.max(1) as i32,
                y0: py as i32,
                x1: now_x,
                y1: y as i32,
                on: true,
            });
            // Time-axis ruler tick every 5 s of travel.
            if sim % (8 * layout::GRID_TICK_S as u64) == 0 {
                panel.apply(Op::Line {
                    x0: now_x,
                    y0: plot_top as i32,
                    x1: now_x,
                    y1: (plot_top + 4) as i32,
                    on: true,
                });
            }
        }

        if sample_now {
            px_since_sample = 0;
        }

        let t1 = timer.get_counter().ticks();
        panel.present();
        let t2 = timer.get_counter().ticks();

        // Pull, then wait for the reply until the frame deadline.
        pull_seq = pull_seq.wrapping_add(1);
        let mut req = LineBuf::new();
        let _ = writeln!(req, "{{\"pull\":{}}}", pull_seq);
        let mut tag = LineBuf::new();
        let _ = write!(tag, "\"pull\":{},", pull_seq);
        if link::send(req.as_bytes()) {
            let deadline = t0 + frame_us;
            loop {
                let now = timer.get_counter().ticks();
                if let Some(n) = link::take_line(&mut reply) {
                    if contains(&reply[..n], tag.as_bytes()) {
                        rtt_us_max = rtt_us_max.max(now - t2);
                        rtt_us_sum += now - t2;
                        replies += 1;
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
        if t2 - t1 > present_us_max {
            present_us_max = t2 - t1;
            present_bytes_at_max = panel.last_flush_bytes;
        }
        bytes_total += panel.last_flush_bytes;
        frames += 1;

        // One line a second: achieved rate, worst-case draw and present, and pixel traffic. This is
        // the number the whole design was aimed at — v1 managed 190 to 340 ms per frame.
        let span = t2 - window_start;
        if span >= TICKS_PER_S {
            rprintln!(
                "fps {}  draw_us {}  present_us {} ({} B)  rows {}  KB/s {}  rtt_us avg {} max {}  ok {} miss {} unsent {}",
                frames as u64 * TICKS_PER_S / span,
                draw_us_max,
                present_us_max,
                present_bytes_at_max,
                panel.last_flush_rows,
                bytes_total as u64 * TICKS_PER_S / span / 1024,
                rtt_us_sum / (replies.max(1) as u64),
                rtt_us_max,
                replies,
                misses,
                unsent
            );
            window_start = t2;
            frames = 0;
            draw_us_max = 0;
            present_us_max = 0;
            present_bytes_at_max = 0;
            bytes_total = 0;
            rtt_us_max = 0;
            rtt_us_sum = 0;
            replies = 0;
            misses = 0;
            unsent = 0;
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

/// True if `needle` occurs in `hay`.
fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// Integer-range sine (Bhaskara I), good enough for a fake curve on a scaffold.
fn sine(x: f32) -> f32 {
    const TWO_PI: f32 = 6.283_185_3;
    let mut a = x - (x / TWO_PI) as i32 as f32 * TWO_PI;
    if a < 0.0 {
        a += TWO_PI;
    }
    let pi = core::f32::consts::PI;
    if a <= pi {
        let n = 16.0 * a * (pi - a);
        let d = 5.0 * pi * pi - 4.0 * a * (pi - a);
        n / d
    } else {
        let b = a - pi;
        let n = 16.0 * b * (pi - b);
        let d = 5.0 * pi * pi - 4.0 * b * (pi - b);
        -(n / d)
    }
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

    panel.apply(Op::Clear { on: true });
    panel.present();
    wait_us(timer, 500_000);

    panel.apply(Op::Clear { on: false });
    panel.apply(Op::Line { x0: 0, y0: 0, x1: 0, y1: h as i32 - 1, on: true });
    panel.present();
    wait_us(timer, 500_000);

    panel.apply(Op::Clear { on: false });
    panel.apply(Op::Line { x0: 0, y0: 0, x1: w as i32 - 1, y1: 0, on: true });
    panel.present();
    wait_us(timer, 500_000);

    panel.apply(Op::Clear { on: false });
    panel.apply(Op::Rect { x: 0, y: 0, w: w / 2, h, fill: true, t: 0, on: true });
    panel.present();
    wait_us(timer, 500_000);

    panel.apply(Op::Clear { on: false });
    panel.apply(Op::Rect { x: 0, y: 0, w: w - 1, h: h - 1, fill: false, t: 1, on: true });
    let mut x: u16 = 0;
    while x < w {
        panel.apply(Op::Line { x0: x as i32, y0: 0, x1: x as i32, y1: h as i32 - 1, on: true });
        x += 50;
    }
    let mut y: u16 = 0;
    while y < h {
        panel.apply(Op::Line { x0: 0, y0: y as i32, x1: w as i32 - 1, y1: y as i32, on: true });
        y += 40;
    }
    panel.present();
    wait_us(timer, 1_500_000);
}

/// Static chrome for the motion demo: the panel border, each chart frame, and a rule through the
/// number slot so the layout is visible before any glyph atlas exists.
fn draw_chrome<P: Panel>(panel: &mut P, rows: &[layout::Row; layout::ROWS]) {
    let (w, h) = panel.size();
    panel.apply(Op::Clear { on: false });
    panel.apply(Op::Rect { x: 0, y: 0, w: w - 1, h: h - 1, fill: false, t: 1, on: true });
    for r in rows {
        panel.apply(Op::Rect {
            x: r.chart_x as i32,
            y: r.chart_y as i32,
            w: r.chart_w(),
            h: r.chart_h,
            fill: false,
            t: 1,
            on: true,
        });
        panel.apply(Op::Line {
            x0: r.slot_x as i32,
            y0: (r.chart_y + r.chart_h / 2) as i32,
            x1: (r.chart_x - 2) as i32,
            y1: (r.chart_y + r.chart_h / 2) as i32,
            on: true,
        });
    }
}
