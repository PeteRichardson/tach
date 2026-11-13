/* RP2350 memory map: 520 KB SRAM, 4 MB QSPI flash (as fitted to a Raspberry Pi Pico 2).
   RAM lengths follow the RP2350 layout: SRAM0-4 are 64 KiB each (0x20000000..0x20080000) and
   SRAM5 is the last 8 KiB. SRAM5 is the region that survives deep sleep.

   No boot stage 2 gap. On RP2040 the application had to sit at 0x10000100, after a 256-byte boot
   stage 2. On RP2350 the boot ROM configures XIP itself: it reads the attached flash's geometry
   from OTP (the CS0_SIZE field of BOOT_FLAGS0 / FLASH_DEVICE_ID, see rp235x_pac::otp_data), so a
   standard QSPI part needs no boot stage 2 binary, and picotool's own default load offset for
   RP2350 is 0x10000000.

   The link address also has to satisfy the ARM vector-table rule: align to the larger of 128 bytes
   and the next power of two at least as large as the table. The RP2350 PAC declares 50 IRQs, so
   the table is 64 + 50*4 = 264 bytes, which needs 512-byte alignment. 0x10000000 satisfies it;
   the RP2040 habit of 0x10000100 does not, and cortex-m-rt refuses to link that. */

MEMORY {
    FLASH  : ORIGIN = 0x10000000, LENGTH = 4096K
    RAM    : ORIGIN = 0x20000000, LENGTH = 520K
}

REGION_ALIAS("CRITICAL_RAM", RAM);
REGION_ALIAS("SLOW_RAM", RAM);
REGION_ALIAS("ARM_CM33_ITCM", RAM);
REGION_ALIAS("ARM_CM33_DTCM", RAM);

/* The RP2350 boot ROM starts an image only if it finds an IMAGE_DEF block in the first 4 KiB of
   flash. Without one, `picotool info` reports "Program Information: none" and the ROM falls back
   to BOOTSEL. The block is the `IMAGE_DEF` static in main.rs; this places it directly after the
   vector table, and moves the start of .text past it so the two do not overlap. Same layout as the
   rp235x-hal examples. */
SECTIONS {
    .start_block : ALIGN(4)
    {
        __start_block_addr = .;
        KEEP(*(.start_block));
        KEEP(*(.boot_info));
    } > FLASH
} INSERT AFTER .vector_table;

_stext = ALIGN(ADDR(.start_block) + SIZEOF(.start_block), 8);
