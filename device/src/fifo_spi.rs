//! The SPI bus after init: one 16-bit frame for each RA8875 access, with chip select in hardware.
//!
//! An RA8875 access is two bytes, a prefix and a data byte, with CS low across both. In SPI mode 0
//! the PL022 raises its own CSn for one clock between frames, so one 16-bit frame is exactly one
//! access, and the CPU does not toggle CS. That one clock of CS high is not enough for the RA8875,
//! so `send` waits for the previous access to leave and adds a pause (see `GAP_CYCLES`).
//!
//! Before this bus, every access set a GPIO CS, sent its two bytes, and waited for the wire.
//! Measured 2026-11-17 at 18.75 MHz: 2000 short lines took 41.5 ms that way, and 32.8 ms on this
//! bus (24.7 ms without the pause).
//!
//! A filled shape of a few pixels can end before the first poll after `flush`, so WAIT is not
//! seen low for it. All such shapes seen in a 100 s log were fills of 8 pixels or less.
//!
//! `send` returns when the access is in the FIFO, not when it is on the wire. Call `flush` before
//! anything that depends on the chip having it, such as a poll of WAIT.

use rp235x_hal::spi::{Enabled, Spi, SpiDevice, ValidSpiPinout};

use crate::ra8875::Link;

/// CPU cycles of CS high between two accesses, more than the one SPI clock that the port gives.
/// With only that one clock (53 ns at 18.75 MHz), about 1 access in 100,000 was lost: a register
/// select that did not take, so its data went to the register selected before. In a 5 minute
/// log, 76 glyph registers read back wrong; with this pause, 0 (measured 2026-11-17). At
/// 150 MHz, 16 cycles is 107 ns, plus the time to see BSY clear.
const GAP_CYCLES: u32 = 16;

/// A configured SPI peripheral, taken out of the HAL driver so that this bus owns its registers.
pub struct FifoSpi<D, P, CS> {
    device: D,
    _pins: P,
    /// The CSn pin, already switched to its SPI function. Held so that nothing else takes it.
    _cs: CS,
}

impl<D: SpiDevice, P: ValidSpiPinout<D>, CS> FifoSpi<D, P, CS> {
    /// Take over an initialised 8-bit HAL bus in mode 0 and its CSn pin. The clock stays as it is;
    /// the frame becomes 16 bits.
    pub fn new(spi: Spi<Enabled, D, P, 8>, cs: CS) -> Self {
        let (device, pins) = spi.free();
        // The data size changes only while the port is off.
        device.sspcr1().modify(|_, w| w.sse().clear_bit());
        device.sspcr0().modify(|_, w| unsafe { w.dss().bits(15) });
        device.sspcr1().modify(|_, w| w.sse().set_bit());
        Self { device, _pins: pins, _cs: cs }
    }
}

impl<D: SpiDevice, P: ValidSpiPinout<D>, CS> Link for FifoSpi<D, P, CS> {
    fn send(&mut self, prefix: u8, byte: u8) {
        // One access at a time, then a pause: see GAP_CYCLES. RX is not read here, so it
        // overruns. Nothing comes back on a write, and flush clears the overrun flag.
        while self.device.sspsr().read().bsy().bit_is_set() {}
        cortex_m::asm::delay(GAP_CYCLES);
        let frame = (prefix as u16) << 8 | byte as u16;
        self.device.sspdr().write(|w| unsafe { w.data().bits(frame) });
    }

    fn exchange(&mut self, prefix: u8, byte: u8) -> u8 {
        self.flush();
        let frame = (prefix as u16) << 8 | byte as u16;
        self.device.sspdr().write(|w| unsafe { w.data().bits(frame) });
        while self.device.sspsr().read().rne().bit_is_clear() {}
        self.device.sspdr().read().data().bits() as u8
    }

    fn flush(&mut self) {
        // Wait for the shift register too, not only an empty FIFO: BSY stays set until the last
        // bit is out. Then drop what RX collected and clear the overrun flag.
        while self.device.sspsr().read().bsy().bit_is_set() {}
        while self.device.sspsr().read().rne().bit_is_set() {
            let _ = self.device.sspdr().read();
        }
        self.device.sspicr().write(|w| w.roric().clear_bit_by_one());
    }
}
