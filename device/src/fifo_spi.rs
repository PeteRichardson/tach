//! An SPI bus that keeps the TX FIFO full, for a device that only takes data.
//!
//! Why this file exists: rp235x-hal 0.4's `SpiBus::write` sends one byte, waits for that byte to
//! come back in RX, then sends the next. The 8-entry FIFO never fills, so there is a gap after
//! every byte. Measured on 5200 B at 10.71 MHz: 4856 us against 3882 us of wire time, 25% lost.
//! This bus fills the FIFO the way the Pico SDK's `spi_write_blocking` does, which measured
//! 3887 us. See device/BRINGUP.md, "present cost measured".
//!
//! `write` returns when the last byte is in the FIFO, not when it is on the wire. That is the
//! embedded-hal contract: call `flush` before you release chip select.

use core::convert::Infallible;

use embedded_hal::spi::{ErrorType, SpiBus};
use rp235x_hal::spi::{Enabled, Spi, SpiDevice, ValidSpiPinout};

/// A configured SPI peripheral, taken out of the HAL driver so that this bus owns its registers.
pub struct FifoSpi<D, P> {
    device: D,
    _pins: P,
}

impl<D: SpiDevice, P: ValidSpiPinout<D>> FifoSpi<D, P> {
    /// Take over an initialised 8-bit HAL bus. Its clock, mode and enable stay as `init` set them.
    pub fn new(spi: Spi<Enabled, D, P, 8>) -> Self {
        let (device, pins) = spi.free();
        Self { device, _pins: pins }
    }

    /// Send one byte and return the byte that came back, with nothing else in flight.
    fn exchange(&mut self, byte: u8) -> u8 {
        let _ = self.flush();
        self.device.sspdr().write(|w| unsafe { w.data().bits(byte as u16) });
        while self.device.sspsr().read().rne().bit_is_clear() {}
        self.device.sspdr().read().data().bits() as u8
    }
}

impl<D, P> ErrorType for FifoSpi<D, P> {
    type Error = Infallible;
}

impl<D: SpiDevice, P: ValidSpiPinout<D>> SpiBus<u8> for FifoSpi<D, P> {
    fn write(&mut self, words: &[u8]) -> Result<(), Infallible> {
        // RX is not read here, so it overruns. The received bytes are of no use on a bus with no
        // MISO, and flush clears the overrun flag.
        for &word in words {
            while self.device.sspsr().read().tnf().bit_is_clear() {}
            self.device.sspdr().write(|w| unsafe { w.data().bits(word as u16) });
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), Infallible> {
        // Wait for the shift register too, not only an empty FIFO: BSY stays set until the last
        // bit is out. Then drop what RX collected and clear the overrun flag.
        while self.device.sspsr().read().bsy().bit_is_set() {}
        while self.device.sspsr().read().rne().bit_is_set() {
            let _ = self.device.sspdr().read();
        }
        self.device.sspicr().write(|w| w.roric().clear_bit_by_one());
        Ok(())
    }

    fn read(&mut self, words: &mut [u8]) -> Result<(), Infallible> {
        for word in words.iter_mut() {
            *word = self.exchange(0);
        }
        Ok(())
    }

    fn transfer(&mut self, read: &mut [u8], write: &[u8]) -> Result<(), Infallible> {
        for i in 0..read.len().max(write.len()) {
            let byte = self.exchange(write.get(i).copied().unwrap_or(0));
            if let Some(r) = read.get_mut(i) {
                *r = byte;
            }
        }
        Ok(())
    }

    fn transfer_in_place(&mut self, words: &mut [u8]) -> Result<(), Infallible> {
        for word in words.iter_mut() {
            *word = self.exchange(*word);
        }
        Ok(())
    }
}
