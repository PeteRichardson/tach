//! Bisect stage 0: does any Rust image of ours run at all? Uses the Pico 2 user LED on GP25 as the
//! only output. No RTT, no SPI, no panel, and nothing that can block or halt the debug link. If the
//! LED pulses, the toolchain, link address and reset path are all good and the fault is lower down.
//!
//! The full milestone-1 program is preserved in src/main_full.rs.

#![no_std]
#![no_main]

use embedded_hal::digital::OutputPin;
use panic_halt as _;

use rp235x_hal::{
    self as hal,
    clocks::init_clocks_and_plls,
    gpio::Pins,
    sio::Sio,
    watchdog::Watchdog,
};

const XOSC_HZ: u32 = 12_000_000;

/// 250 ms per half cycle: slow enough to see clearly, fast enough to prove the loop is live.
const HALF_US: u64 = 250_000;

/// Tell the RP2350 boot ROM that this is a secure Arm executable. Without this block in the
/// first 4 KiB of flash the ROM does not start the image and drops back to BOOTSEL.
#[link_section = ".start_block"]
#[used]
pub static IMAGE_DEF: hal::block::ImageDef = hal::block::ImageDef::secure_exe();

#[hal::entry]
fn main() -> ! {
    let Some(pac) = hal::pac::Peripherals::take() else {
        // Unreachable on a clean boot: nothing else has taken the peripherals.
        loop {
            core::hint::spin_loop();
        }
    };

    let mut resets = pac.RESETS;
    let mut watchdog = Watchdog::new(pac.WATCHDOG);

    let sio = Sio::new(pac.SIO);
    let mut pins = Pins::new(pac.IO_BANK0, pac.PADS_BANK0, sio.gpio_bank0, &mut resets);
    let mut led = pins.gpio25.into_push_pull_output();

    // If the clocks refuse to lock, blink on a cycle count instead of halting. hal::halt() turns
    // core 1 off and costs the SWD link, which is precisely the failure mode that made the earlier
    // stages unreadable.
    let Ok(clocks) = init_clocks_and_plls(
        XOSC_HZ,
        pac.XOSC,
        pac.CLOCKS,
        pac.PLL_SYS,
        pac.PLL_USB,
        &mut resets,
        &mut watchdog,
    ) else {
        loop {
            for _ in 0..20_000_000 {
                core::hint::spin_loop();
            }
            let _ = led.set_high();
            for _ in 0..20_000_000 {
                core::hint::spin_loop();
            }
            let _ = led.set_low();
        }
    };

    let mut timer = hal::timer::Timer::new_timer0(pac.TIMER0, &mut resets, &clocks);

    loop {
        let start = timer.get_counter().ticks();
        let _ = led.set_high();
        while timer.get_counter().ticks() - start < HALF_US {
            core::hint::spin_loop();
        }
        let start = timer.get_counter().ticks();
        let _ = led.set_low();
        while timer.get_counter().ticks() - start < HALF_US {
            core::hint::spin_loop();
        }
    }
}
