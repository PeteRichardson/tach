# The Mac clock times the chart, and the rate goes on the wire

The Mac stamps every message with its own monotonic clock in milliseconds and announces the
sample rate once when the device connects. The device uses those stamps to place samples in time
and uses its own clock only for frame pacing. It does not discipline its clock against the Mac:
the residual drift of a crystal against a host clock is tens of parts per million, which moves a
column by well under one pixel per minute.

This is the mechanism that lets a 2 Hz stream drive smooth motion, and it replaces the unworked
assumptions in issue #10 ("protocol rate is not agreed on the wire").

## Rules

- Rate at connect: the device must know the sample interval before it can place a sample, so a
  connect-time message carries it. Adding unknown keys is what the protocol already permits, so
  v1 firmware ignores it.
- Stamps are monotonic, not wall-clock, so a clock adjustment cannot drag the chart backwards.
- If stamps stop advancing for more than a few sample intervals — Mac sleep, a hung sender — the
  device blanks the panel instead of freezing, which is issue #1 and issue #27 solved as one rule
  in one place.

## Consequences

- Column position comes from `(stamp_now − stamp_sample) / column_span`, not from a shift counter.
- Device reboot while the Mac keeps sending needs no special handshake beyond the connect message.
- The Mac's monotonic clock and the device's clock share no epoch, so the device never shows
  absolute times.
- The exact JSON keys are still open; see `docs/device-v2.md` open frontier item 3.
