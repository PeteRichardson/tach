//! The USB CDC link to the Mac: one pull out, one reply line back (ADR-0006).
//!
//! The USB stack runs in the USBCTRL interrupt, not in the frame loop. The loop busy-waits for
//! whole milliseconds (present is about 7 ms), and the host needs answers on the control endpoint
//! faster than the loop could give them. The interrupt collects bytes into lines; the loop takes
//! the newest complete line. An older line that the loop did not take is replaced, because only
//! the newest sample is of use.

use core::cell::RefCell;

use critical_section::Mutex;
use rp235x_hal::{self as hal, pac, pac::interrupt};
use static_cell::StaticCell;
use usb_device::{bus::UsbBusAllocator, prelude::*};
use usbd_serial::SerialPort;

/// Longest line the link keeps. A full Q36 reply is about 200 B. A longer line is dropped whole.
pub const LINE_MAX: usize = 256;

/// pid.codes test VID/PID for CDC-ACM, the pair the rp-hal examples use. Change it before more
/// than one of these devices exists.
const VID_PID: UsbVidPid = UsbVidPid(0x16c0, 0x27dd);

static BUS: StaticCell<UsbBusAllocator<hal::usb::UsbBus>> = StaticCell::new();
static LINK: Mutex<RefCell<Option<Link>>> = Mutex::new(RefCell::new(None));

struct Link {
    dev: UsbDevice<'static, hal::usb::UsbBus>,
    serial: SerialPort<'static, hal::usb::UsbBus>,
    /// The line that is still arriving.
    acc: [u8; LINE_MAX],
    acc_len: usize,
    /// True after a line ran past LINE_MAX, until its newline: the rest of it is dropped.
    overlong: bool,
    /// The newest complete line, without its newline.
    line: [u8; LINE_MAX],
    line_len: usize,
    line_ready: bool,
}

/// Start the USB device and let its interrupt run. Call once.
pub fn init(
    usb: pac::USB,
    dpram: pac::USB_DPRAM,
    clock: hal::clocks::UsbClock,
    resets: &mut pac::RESETS,
) {
    let bus = BUS.init(UsbBusAllocator::new(hal::usb::UsbBus::new(
        usb, dpram, clock, true, resets,
    )));
    let serial = SerialPort::new(bus);
    let Ok(builder) = UsbDeviceBuilder::new(bus, VID_PID).strings(&[StringDescriptors::default()
        .manufacturer("tach")
        .product("tach device")
        .serial_number("scaffold")])
    else {
        hal::halt()
    };
    let dev = builder.device_class(usbd_serial::USB_CLASS_CDC).build();

    critical_section::with(|cs| {
        LINK.borrow(cs).replace(Some(Link {
            dev,
            serial,
            acc: [0; LINE_MAX],
            acc_len: 0,
            overlong: false,
            line: [0; LINE_MAX],
            line_len: 0,
            line_ready: false,
        }));
    });
    // SAFETY: LINK is set above, so the handler has its state before the first interrupt.
    unsafe { cortex_m::peripheral::NVIC::unmask(pac::Interrupt::USBCTRL_IRQ) };
}

/// Send bytes to the host. False if the host has not opened the port (DTR low) or the USB
/// buffer has no room: the caller skips this pull and tries on the next frame.
pub fn send(bytes: &[u8]) -> bool {
    critical_section::with(|cs| {
        let mut link = LINK.borrow_ref_mut(cs);
        let Some(link) = link.as_mut() else { return false };
        if !link.serial.dtr() {
            return false;
        }
        // Drop a reply to an earlier pull, so the next line taken answers this one.
        link.line_ready = false;
        matches!(link.serial.write(bytes), Ok(n) if n == bytes.len())
    })
}

/// Copy out the newest complete line, if one came in since the last call.
pub fn take_line(out: &mut [u8; LINE_MAX]) -> Option<usize> {
    critical_section::with(|cs| {
        let mut link = LINK.borrow_ref_mut(cs);
        let link = link.as_mut()?;
        if !link.line_ready {
            return None;
        }
        link.line_ready = false;
        out[..link.line_len].copy_from_slice(&link.line[..link.line_len]);
        Some(link.line_len)
    })
}

#[interrupt]
fn USBCTRL_IRQ() {
    critical_section::with(|cs| {
        let mut link = LINK.borrow_ref_mut(cs);
        let Some(link) = link.as_mut() else { return };
        if !link.dev.poll(&mut [&mut link.serial]) {
            return;
        }
        let mut buf = [0u8; 64];
        while let Ok(n @ 1..) = link.serial.read(&mut buf) {
            for &b in &buf[..n] {
                link.push(b);
            }
        }
    });
}

impl Link {
    fn push(&mut self, b: u8) {
        match b {
            b'\n' => {
                if !self.overlong {
                    self.line[..self.acc_len].copy_from_slice(&self.acc[..self.acc_len]);
                    self.line_len = self.acc_len;
                    self.line_ready = true;
                }
                self.acc_len = 0;
                self.overlong = false;
            }
            b'\r' => {}
            _ if self.acc_len == LINE_MAX => self.overlong = true,
            _ => {
                self.acc[self.acc_len] = b;
                self.acc_len += 1;
            }
        }
    }
}
