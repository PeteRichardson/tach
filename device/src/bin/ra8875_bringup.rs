//! RA8875 bring-up: prove the wiring in device/WIRING.md before the firmware moves to the new
//! panel. Resets the chip, reads its ID register, sets 480x272 timing, turns the backlight on and
//! fills the screen with red, green, blue and white bars. Results go out over RTT.
//!
//! Run: `cargo run --release --bin ra8875_bringup`
//!
//! Register values follow the Adafruit_RA8875 library's 480x272 set-up.

#![no_std]
#![no_main]

use panic_halt as _;

use embedded_hal::{
    digital::{InputPin, OutputPin},
    spi::{SpiBus, MODE_0, MODE_3},
};
use fugit::RateExtU32;
use rp235x_hal::{
    self as hal,
    clocks::init_clocks_and_plls,
    gpio::{FunctionSpi, Pins},
    sio::Sio,
    spi::Spi,
    watchdog::Watchdog,
    Clock,
};
use rtt_target::{rprintln, rtt_init_print};

const XOSC_HZ: u32 = 12_000_000;
/// The RA8875 PLL is not running at reset, so the bus must be slow until it is set.
const SPI_INIT_HZ: u32 = 1_000_000;
/// After the PLL is set. Adafruit's CircuitPython driver found 6 MHz stable.
const SPI_RUN_HZ: u32 = 6_000_000;

const W: u16 = 480;
const H: u16 = 272;

/// SPI prefix bytes: the first byte of every transaction says what the second one is.
const CMD_WRITE: u8 = 0x80;
const DATA_WRITE: u8 = 0x00;
const DATA_READ: u8 = 0x40;

/// The value of register 0x00 on a live RA8875.
const CHIP_ID: u8 = 0x75;

#[link_section = ".start_block"]
#[used]
pub static IMAGE_DEF: hal::block::ImageDef = hal::block::ImageDef::secure_exe();

struct Ra<S, C> {
    spi: S,
    cs: C,
}

impl<S: SpiBus, C: OutputPin> Ra<S, C> {
    fn xfer(&mut self, prefix: u8, byte: u8) -> u8 {
        let mut buf = [prefix, byte];
        let _ = self.cs.set_low();
        let _ = self.spi.transfer_in_place(&mut buf);
        let _ = self.spi.flush();
        let _ = self.cs.set_high();
        buf[1]
    }

    fn write_reg(&mut self, reg: u8, val: u8) {
        self.xfer(CMD_WRITE, reg);
        self.xfer(DATA_WRITE, val);
    }

    fn read_reg(&mut self, reg: u8) -> u8 {
        self.xfer(CMD_WRITE, reg);
        self.xfer(DATA_READ, 0)
    }
}

fn wait_us(timer: &hal::timer::Timer<hal::timer::CopyableTimer0>, us: u64) {
    let end = timer.get_counter().ticks() + us;
    while timer.get_counter().ticks() < end {
        core::hint::spin_loop();
    }
}

#[hal::entry]
fn main() -> ! {
    rtt_init_print!();

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
    let timer = hal::timer::Timer::new_timer0(pac.TIMER0, &mut resets, &clocks);

    // CS first, so the chip stays deselected while the rest comes up. The 10k pull-up holds it
    // high until here.
    let mut cs = pins.gpio17.into_push_pull_output();
    let _ = cs.set_high();
    let mut rst = pins.gpio15.into_push_pull_output();
    let mut int = pins.gpio14.into_pull_up_input();
    let mut wait = pins.gpio13.into_pull_up_input();

    let mosi = pins.gpio19.into_function::<FunctionSpi>();
    let miso = pins.gpio16.into_function::<FunctionSpi>();
    let sck = pins.gpio18.into_function::<FunctionSpi>();
    let spi = Spi::<_, _, _, 8>::new(pac.SPI0, (mosi, miso, sck)).init(
        &mut resets,
        clocks.peripheral_clock.freq(),
        SPI_INIT_HZ.Hz(),
        MODE_0,
    );

    rprintln!("ra8875 bring-up");

    let _ = rst.set_low();
    wait_us(&timer, 100_000);
    let _ = rst.set_high();
    wait_us(&timer, 100_000);
    rprintln!(
        "after reset: INT {} WAIT {} (both should be high)",
        int.is_high().unwrap_or(false) as u8,
        wait.is_high().unwrap_or(false) as u8
    );

    // The guide does not give the SPI mode. Try mode 0 first, as the Adafruit library does, then
    // mode 3.
    let mut ra = Ra { spi, cs };
    let mut id = ra.read_reg(0x00);
    rprintln!("ID in SPI mode 0: {:#04x} (want {:#04x})", id, CHIP_ID);
    if id != CHIP_ID {
        ra.spi.set_format(MODE_3.into());
        id = ra.read_reg(0x00);
        rprintln!("ID in SPI mode 3: {:#04x}", id);
    }
    if id != CHIP_ID {
        rprintln!("FAIL: no RA8875 on the bus. 0x00 = MISO stuck low or CS not reaching the chip; 0xff = MISO floating or chip not powered.");
        loop {
            cortex_m::asm::wfi();
        }
    }

    // PLL: input 20 MHz crystal, so these give the system clock the 480x272 timing expects.
    ra.write_reg(0x88, 0x0A); // PLLC1: PLLDIV1 + 10
    wait_us(&timer, 1_000);
    ra.write_reg(0x89, 0x02); // PLLC2: DIV4
    wait_us(&timer, 1_000);
    ra.write_reg(0x10, 0x0C); // SYSR: 16 bpp, 8-bit MCU interface

    // Pixel clock and panel timing.
    ra.write_reg(0x04, 0x82); // PCSR: falling-edge fetch, system clock / 4
    wait_us(&timer, 1_000);
    ra.write_reg(0x14, (W / 8 - 1) as u8); // HDWR: width
    ra.write_reg(0x15, 0x00); // HNDFTR: DE high, no fine tune
    ra.write_reg(0x16, 1); // HNDR: (10 - 2) / 8
    ra.write_reg(0x17, 0); // HSTR: 8 / 8 - 1
    ra.write_reg(0x18, 5); // HPWR: 48 / 8 - 1, sync active low
    ra.write_reg(0x19, ((H - 1) & 0xFF) as u8); // VDHR0
    ra.write_reg(0x1A, ((H - 1) >> 8) as u8); // VDHR1
    ra.write_reg(0x1B, 2); // VNDR0: 3 - 1
    ra.write_reg(0x1C, 0); // VNDR1
    ra.write_reg(0x1D, 7); // VSTR0: 8 - 1
    ra.write_reg(0x1E, 0); // VSTR1
    ra.write_reg(0x1F, 9); // VPWR: 10 - 1, sync active low

    // Active window: the whole panel.
    ra.write_reg(0x30, 0);
    ra.write_reg(0x31, 0);
    ra.write_reg(0x32, 0);
    ra.write_reg(0x33, 0);
    ra.write_reg(0x34, ((W - 1) & 0xFF) as u8);
    ra.write_reg(0x35, ((W - 1) >> 8) as u8);
    ra.write_reg(0x36, ((H - 1) & 0xFF) as u8);
    ra.write_reg(0x37, ((H - 1) >> 8) as u8);

    // Clear the whole memory.
    ra.write_reg(0x8E, 0x80);
    wait_us(&timer, 500_000);

    // The PLL runs now, so the bus can go faster.
    let hz = ra
        .spi
        .set_baudrate(clocks.peripheral_clock.freq(), SPI_RUN_HZ.Hz())
        .to_Hz();
    rprintln!("SPI now {} Hz", hz);
    let id = ra.read_reg(0x00);
    rprintln!("ID at run speed: {:#04x}", id);

    ra.write_reg(0x01, 0x80); // PWRR: display on
    ra.write_reg(0xC7, 0x01); // GPIOX high: the board uses it as the panel's DISP enable
    ra.write_reg(0x8A, 0x80 | 0x0A); // P1CR: PWM1 on, clock / 1024
    ra.write_reg(0x8B, 0xFF); // P1DCR: full brightness
    rprintln!("display on, backlight PWM1 at 255");

    // Read back: a register that does not hold its value means the write did not arrive.
    for (reg, name) in [
        (0x88u8, "PLLC1"),
        (0x89, "PLLC2"),
        (0x10, "SYSR"),
        (0x04, "PCSR"),
        (0x14, "HDWR"),
        (0x01, "PWRR"),
        (0xC7, "GPIOX"),
        (0x8A, "P1CR"),
        (0x8B, "P1DCR"),
    ] {
        rprintln!("  {} ({:#04x}) = {:#04x}", name, reg, ra.read_reg(reg));
    }

    // Blink the backlight: off and on, once a second, 5 times. If the glass never changes, the
    // backlight does not follow PWM1.
    for n in 0..5 {
        ra.write_reg(0x8B, 0x00);
        rprintln!("backlight OFF ({})", n + 1);
        wait_us(&timer, 1_000_000);
        ra.write_reg(0x8B, 0xFF);
        rprintln!("backlight ON ({})", n + 1);
        wait_us(&timer, 1_000_000);
    }

    let bars: [(u16, &str); 4] = [
        (0xF800, "red"),
        (0x07E0, "green"),
        (0x001F, "blue"),
        (0xFFFF, "white"),
    ];
    let bar_w = W / bars.len() as u16;
    for (i, (color, name)) in bars.iter().enumerate() {
        let x0 = i as u16 * bar_w;
        let x1 = x0 + bar_w - 1;
        ra.write_reg(0x91, (x0 & 0xFF) as u8);
        ra.write_reg(0x92, (x0 >> 8) as u8);
        ra.write_reg(0x93, 0);
        ra.write_reg(0x94, 0);
        ra.write_reg(0x95, (x1 & 0xFF) as u8);
        ra.write_reg(0x96, (x1 >> 8) as u8);
        ra.write_reg(0x97, ((H - 1) & 0xFF) as u8);
        ra.write_reg(0x98, ((H - 1) >> 8) as u8);
        ra.write_reg(0x63, ((color & 0xF800) >> 11) as u8);
        ra.write_reg(0x64, ((color & 0x07E0) >> 5) as u8);
        ra.write_reg(0x65, (color & 0x001F) as u8);
        ra.write_reg(0x90, 0xB0); // DCR: draw a filled rectangle

        // Wait for the drawing engine. Bit 7 of DCR stays set while it runs.
        let start = timer.get_counter().ticks();
        let mut wait_low = false;
        while ra.read_reg(0x90) & 0x80 != 0 {
            wait_low |= !wait.is_high().unwrap_or(true);
            if timer.get_counter().ticks() - start > 100_000 {
                rprintln!("FAIL: {} bar did not finish in 100 ms", name);
                break;
            }
        }
        rprintln!(
            "{} bar drawn in {} us, WAIT seen low: {}",
            name,
            timer.get_counter().ticks() - start,
            wait_low
        );
    }

    // Touch: auto mode, with the touch interrupt routed to INT.
    ra.write_reg(0x70, 0x80 | 0x30 | 0x08 | 0x02); // TPCR0: on, 4096-clock wait, wake, ADC clk / 4
    ra.write_reg(0x71, 0x04); // TPCR1: auto mode, debounce
    let intc1 = ra.read_reg(0xF0);
    ra.write_reg(0xF0, intc1 | 0x04); // INTC1: touch interrupt on
    ra.write_reg(0xF1, 0x04); // INTC2: clear any old touch interrupt

    rprintln!("done. Expect red, green, blue, white bars, left to right. Touch the panel to test INT.");
    loop {
        if int.is_low().unwrap_or(false) {
            // Raw 10-bit ADC readings: high 8 bits in TPXH/TPYH, low 2 bits each in TPXYL.
            let xh = ra.read_reg(0x72) as u16;
            let yh = ra.read_reg(0x73) as u16;
            let xyl = ra.read_reg(0x74) as u16;
            let x = (xh << 2) | (xyl & 0x03);
            let y = (yh << 2) | ((xyl >> 2) & 0x03);
            rprintln!("touch: raw x {} y {}", x, y);
            ra.write_reg(0xF1, 0x04); // clear, so INT goes high again
            wait_us(&timer, 100_000);
        }
        wait_us(&timer, 10_000);
    }
}
