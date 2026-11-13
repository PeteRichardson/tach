# Bring-up log

## Flash layout — verified, not guessed

`memory.x` links at `0x10000000`. Two independent confirmations:

- `picotool load` defaults to `0x10000000` on RP2350.
- `picotool info` on the official MicroPython `RPI_PICO2-20260824-v1.29.0.uf2`:
  `binary start: 0x10000000`, `boot2_name: boot2_w25q080`, SDK 2.3.0. Its first words are
  `00 20 08 20` (SP = `0x20082000`) then reset `0x1000015b`, i.e. the vector table is at the very
  start of flash.

The RP2040 habit of `0x10000100` is wrong here, and cortex-m-rt rejects it: the PAC declares 50
IRQs, so the table is `64 + 50*4 = 264` bytes and ARM requires 512-byte alignment. `0x100` is only
256-aligned. This was the original link failure.

## Current state

- Firmware builds clean for `thumbv8m.main-none-eabihf`.
- `picotool load target/tach_device.bin -o 0x10000000 -f` writes and verifies; `picotool reboot`
  reports application mode.
- `probe-rs run --chip RP235x` reports an exception immediately, with garbage frames. The reported
  PC `0x88` is inside the boot ROM region, but the unwinder is unreliable here, so that is a hint
  and not a finding.
- `probe-rs attach` prints nothing, which is consistent with the firmware never reaching
  `rtt_init_print!`.

## Open, in order

1. Get the real fault: CFSR `0xE000ED28`, HFSR `0xE000ED2C`, and the stacked PC from the exception
   frame. `probe-rs debug` piped with `read32`/`regs` produced no output and needs its invocation
   checked.
2. Bisect `main` by stages, checking with the panel each time, since the panel is the only
   indicator that does not depend on the debugger: clocks only, then GPIO, then SPI init, then the
   first SHARP command.
3. RTT needs the probe: Raspberry Pi Debug Probe, `2e8a:000c`, on SWDIO GP20 / SWCLK GP21 / GND.

## Toolchain trap

Homebrew's `cargo` shadows rustup's and cannot see embedded targets. Prefix builds with
`PATH="$HOME/.cargo/bin:$PATH"`.

## Bisect result

Panel shows **nothing at all** — no pattern, no dimming. So the firmware dies at or before clock
init. The SPI path, the SHARP driver and the layout code are still untested and are not yet
implicated. Next step is to reduce `main` to the smallest thing that can print or light a pixel,
then add stages back one at a time.

## 2026-02-10: the panel is not the problem

The ROM is refusing our image. Evidence, in order of strength:

1. `picotool info -a` on the device: `Program Information: none`, chip RP2350 rev A2, QFN60.
2. The `RP2350` BOOTSEL volume is present again after `picotool load` + `picotool reboot -a`
   (default is an application reboot), so the ROM fell back to BOOTSEL instead of launching.
3. `picotool save -r 0x10000000 0x10000100` reads back byte-identical to `target/tach_device.bin`.
   The image is correctly in flash; it is simply not being started.
4. The Pico 2 user LED (GP25) on a blink-only program: dark. Nothing of ours executes.

Ruled out by measurement, not opinion:

- **SWD wiring.** A swapped SWDIO/SWCLK makes SWD impossible. `probe-rs` enumerated the full
  CoreSight ROM table, identified the Cortex-M33, and wrote flash over SWD. Wiring is correct.
- **Initial stack pointer.** Ours was `0x20080000`, MicroPython's is `0x20082000`. Raised the RAM
  region to 520K so SP = `0x20082000`, rebuilt, reflashed: ROM still refuses. Not the cause.
- **Flash link address.** MicroPython's working image has its vector table at flash offset 0 too.
- **The panel, SPI, and all of `main_full.rs`.** Never reached.

Corrected mistake: earlier I reported "RAM reads all zeros, so our code never ran" as evidence from
`probe-rs read`. That read halts the core, and XIP is not served while halted, so zeros were a tool
artefact. The conclusion happened to be right, but that particular evidence was invalid. The valid
evidence is items 1 to 4 above.

## Next experiment, in order

1. `picotool uf2 convert target/tach_device.bin out.uf2 --family rp2350-arm-s`, then drop `out.uf2`
   on the RP2350 drive. MicroPython arrived as a family-tagged UF2 and boots; ours arrived as a raw
   BIN. If the UF2 route boots, the difference is in how the image is loaded, not in the image.
2. If that fails, test the MBR/PICOF hypothesis: check `picotool help build` and `picotool info
   --mbr` for RP2350 partition-table support, and compare against the first sector of a working
   RP2350 image saved with `picotool save -r 0x10000000 0x10001000`.
3. Copy a known-good Rust Pico 2 `memory.x` and `.cargo/config.toml` verbatim from a project that
   is known to boot on a Pico 2, rather than continuing to vary ours one field at a time.

## 2026-11-12: root cause found — no IMAGE_DEF block. The ROM now boots the image.

The RP2350 boot ROM starts an image only if it finds a valid IMAGE_DEF block (a "picobin" block)
in the first 4 KiB of flash. Our image did not have one, so `picotool info` said
`Program Information: none` and the ROM went back to BOOTSEL. The flash address, stack pointer and
wiring were correct. The block was the only missing part. MicroPython boots because the SDK always
adds this block.

Fix, the same as the rp235x-hal examples:

- `src/main.rs` and `src/main_full.rs`: a `#[link_section = ".start_block"] #[used]` static
  `IMAGE_DEF = hal::block::ImageDef::secure_exe()`.
- `memory.x`: a `.start_block` section `INSERT AFTER .vector_table`, and `_stext` moved past it, so
  `.text` does not overlap the block.

Evidence that it works:

1. `rust-objdump -h`: `.start_block` is 0x14 bytes at `0x10000110`, inside the first 4 KiB.
2. `picotool info -a` after load: `image type: ARM Secure`, metadata block `image def` at
   `0x10000110`.
3. After `picotool reboot`, there is no `RP2350` BOOTSEL volume.
4. DHCSR `0x05100001`: the core runs, is not halted and is not in lockup.
5. GPIO25_STATUS (`0x400280C8`) changes between `0x00002000` and `0x04022200` from sample to
   sample, and the user LED blinks.

Load command that works: `picotool load -t elf <elf> -f`, then `picotool reboot -f`. picotool needs
the `.elf` extension or `-t elf`.

## Next

1. Put `src/main_full.rs` back as the binary and continue the bisect from BRINGUP "Open" item 2:
   clocks, GPIO, SPI init, first SHARP command.
2. With the image now running, `probe-rs attach` with RTT should show output. Check this first.

## 2026-11-12: full firmware runs to the frame loop

`src/main_full.rs` is `src/main.rs` again. The blink program is kept as `src/main_blink.rs` (not
compiled). `_stext` is now aligned to 8 bytes (`ALIGN(..., 8)` in `memory.x`); before this, the
linker warned that `.text` at `0x10000124` was misaligned.

Flash and RTT through the probe: `probe-rs run --chip RP235x <elf>`. picotool cannot reboot this
firmware into BOOTSEL, because the firmware has no USB interface.

RTT output:

```
tach scaffold up: 400x240 @ 12000000 Hz SPI
fps 29  draw_us 327  present_us 9379  rows 0  KB/s 113
```

So clocks, GPIO, SPI init, the self-test and the frame loop all run. The firmware does not stop
anywhere. The old "Open" item 2 bisect is not necessary.

Notes for later, not faults:

- Worst `present_us` is 9379 µs, every second. At 12 MHz, a full 12 KB frame needs about 8.3 ms,
  but 113 KB/s at 30 fps is only about 3.8 KB per frame. The per-row SPI write cost is possibly
  high. Measure this before the SPI clock goes up.
- `rows` in the log is the row count of the last frame only, so it is often 0. It is not a sum.

Still open: what the glass shows for the five self-test patterns and the chrome.

## 2026-11-12: glass verified, two motion-demo bugs fixed

User report: the border, three chart frames and three moving curves show correctly on the
glass. Thus power, CS polarity, VCOM, SPI mode, bit order and the 400x240 geometry are correct.

Bugs found from what the glass showed:

1. **Whole chart frames moved and wrapped.** `shift_band_left` called `[u8]::rotate_left(1)` on a
   row. That turns the row by one *byte* (8 px), and the dropped byte comes back on the right. The
   band was also the full panel width, so the frame edges moved with the curve. Fix: a real
   pixel shift with no wrap, limited to a window. `Op::ShiftBand` now has `left` and `width`
   (the RA8875 scroll window has horizontal limits too). The loop shifts only `Row::plot()`, which
   is the inside of the 1 px frame. Samples and ticks are drawn inside the frame, not on its
   right edge.
2. **Only the first sample was drawn.** `sample_now = phase_x100 / 100 >= sim` was always false
   after the first frame, because `phase_x100` stays below 100 after `%= 100`. Fix: an 8 Hz
   sample clock from the 1 MHz timer (`SAMPLE_HZ`). Each sample joins the previous one over the
   real distance that the plot moved since that sample.

Shift cost: per-pixel loop 7.6 ms worst `draw_us`; byte-wise shift with masks 2.0 ms. The
byte-wise version gave the same result as the per-pixel loop in 200,000 random cases (windows,
shifts 0–19, both background levels) in a test on the Mac before it went on the device.

Current RTT: `fps 29  draw_us 1992  present_us 9079  KB/s 111`.

## Next

1. `present_us` 9.1 ms for about 3.7 KB per frame is slow. That is about 3x the wire time at 12 MHz.
   Probably the cost is one blocking `spi.write` per row. Measure one full-frame flush, then try a
   single write for all rows, before the SPI clock goes up.
2. `draw_us` 2.0 ms for about 9,000 byte steps is about 33 cycles per byte. This is good enough
   for now. Check `opt-level` and bounds checks if it becomes important.
3. Tidy the dead-code warnings (`Madrs` variants, `Brightness`, `vcom_toggle`, `CMD_CLEAR_MEMORY`)
   when the protocol work uses or removes them.

## 2026-11-12: present cost measured — 1.3x wire time, not 3x

The "3x" above was wrong. It divided the *worst* present time by the *average* bytes per frame. The
worst frame is the frame that shifts the plots. That frame sends all 180 plot rows: 9362 B.

Two facts from the rp235x-hal 0.4 source:

- **The SPI clock is 10.71 MHz, not 12 MHz.** `set_baudrate` gives
  `clk_peri / (prescale * (1 + postdiv))` at or below the request: 150 MHz / (2 * 7).
- **`SpiBus::write` sends one byte at a time.** For each byte it writes TX, then waits for that
  byte to come back in RX. The 8-entry FIFO is never full, so there is a gap after each byte.

Benchmark at boot, 5200 B (100 rows of 52), CS low so that the panel ignores the data:

| Path | Time | Compared to wire time |
| --- | --- | --- |
| Wire time at 10.71 MHz | 3882 us | 1.00x |
| One `spi.write` of 5200 B | 4856 us | 1.25x |
| 100 `spi.write` of 52 B (what `flush` does) | 4863 us | 1.25x |
| Raw loop that keeps the TX FIFO full (Pico SDK `spi_write_blocking`) | 3887 us | 1.00x |

Frame loop: worst present 9079 us for 9362 B. Wire time 6990 us, so 1.30x.

Where the 9079 us goes:

- 6990 us: wire time at 10.71 MHz.
- About 1750 us: the byte-at-a-time gap (25%). The cost per call is too small to see.
- About 340 us: the rest of `flush`. That is the CS edges, the dirty-flag scan and the copy of
  each row into its record.

Options, not done yet:

1. A FIFO-full write gives a present of about 7.3 ms. It needs raw register access, for example
   a small wrapper around the HAL `Spi` that implements `SpiBus::write` the way the SDK does.
2. The clock: at 150 MHz, the HAL can give 10.71 MHz (at most 12 MHz) or 12.5 MHz (`SPI_HZ` set
   to 12.5 MHz). The LS027B7DH01 datasheet limit is much lower, so this needs a test on the glass.
3. The volume: each shift sends the full plot area again, because the SHARP panel has no scroll.
   This is the largest cost, but the RA8875 does not have it (hardware scroll). Thus it is
   possibly not worth optimizing on the scaffold.

The `present_us` log now also shows the bytes of the worst frame.

## 2026-11-12: FIFO-full SPI write — present at wire speed

Option 1 is done. `src/fifo_spi.rs` takes the configured HAL `Spi` with `free()` and implements
`SpiBus<u8>`. `write` keeps the TX FIFO full and returns when the last byte is in the FIFO.
`flush` waits for BSY to clear, drains RX and clears the overrun flag.

The driver did not call `spi.flush()` before it set CS low. That worked only because the HAL write
waited for each byte. All four CS releases in `sharp.rs` now call `flush()` first, as the
embedded-hal contract requires.

Result: worst present 7000 us for 9362 B, wire time 6990 us, so 1.00x (was 9079 us, 1.30x). The
rest of `flush` (about 340 us) is also gone, because the CPU prepares the next row while the FIFO
sends the current row. The user confirmed the image on the glass is correct. The boot log now
shows the real clock: `400x240 @ 10714285 Hz SPI (12000000 Hz requested)`. The benchmark is
removed.

Still open from the options above: 12.5 MHz (test on the glass), and the shift volume.

## 2026-11-12: 12.5 MHz SPI fails — 10.71 MHz is the ceiling at clk_peri 150 MHz

`SPI_HZ = 12_500_000` gives exactly 12.5 MHz (prescale 2, postdiv 5). The worst present went to
6001 us for 9362 B, as calculated, but the panel was **blank**. The core ran (DHCSR `0x05100001`)
and RTT showed 30 fps, so the firmware was not the problem. When I set it back to 12 MHz requested
(10.71 MHz), with no other change, the image came back.

From a 150 MHz clk_peri with prescale 2, the rates are 150 / (2 * (1 + postdiv)): 12.5 MHz, then
10.71 MHz. There is nothing between them, so 10.71 MHz is the highest rate that works with this
clock source. It is already far above the LS027B7DH01 datasheet figure.

Not tried: exactly 12 MHz. That needs a different clk_peri, for example 48 MHz from PLL_USB
(48 / (2 * 2) = 12 MHz). It could give at most 11% on a panel that the RA8875 will replace, and
it has less margin than 10.71 MHz, which works.

## 2026-11-12: USB CDC up — pull round trip measured

`device/src/link.rs` runs `usb-device` 0.3 and `usbd-serial` 0.2 on the RP2350 USB controller.
The stack runs in the `USBCTRL_IRQ` handler, because the frame loop busy-waits for 7 ms in present
and the host expects quick answers on the control endpoint. The handler joins bytes into lines; the
loop takes the newest complete line.

The device enumerates as `tach device`, VID:PID `16c0:27dd` (the pid.codes test pair for CDC-ACM
that the rp-hal examples use), serial number `scaffold`. On macOS the port is
`/dev/cu.usbmodemscaffold1`. It is a different VID from the PyPortal (`239a`), so `tach-sender.py`
does not find it yet.

Probe: after each present the loop sends `{"pull":N}` and spins until a line with `"pull":N,`
comes back, or until the frame deadline. A test responder on the Mac (Python, pyserial) answered
each pull with a Q36-shaped reply of about 190 B. 60 s at 30 fps:

| Pulls answered | Missed | Round trip, average | Round trip, worst |
| --- | --- | --- | --- |
| 1789 | 0 | 494 us | 869 us |

The frame rate stayed at 29–30 fps, and draw and present did not change (2.0 ms, 7.0 ms). The
frame budget at 30 fps is now about 2.0 + 7.0 + 0.9 = 9.9 ms of 33.3 ms.

Until the Mac opens the port (DTR low), the device does not send, and counts the pull as `unsent`.

Not settled yet: the request format. `{"pull":N}` is a probe, so that a late reply cannot be
taken for the answer to the next pull. The Q36 reply keys have no sequence number.
