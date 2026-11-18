# The CPU row shows one bar per core

The CPU row is a bar chart with one bar for each core, not a curve over time. Each bar is a stack
of LED segments, lit up to the core's level, with a peak marker above it, as on an audio spectrum
analyser. The GPU and memory rows keep their curves. Issue #46.

- The bars fill the plot's width. The fastest cluster type (on the M5 Ultra, the 12 super cores)
  is on the left in cyan; the other cores follow in violet. Every core has the same full height,
  and every bar is red at its core's own 100 %.
- The level is a 100 ms EMA of the core's busy percent. The peak takes the raw value, so a spike
  shorter than the EMA still shows. It holds for 600 ms, then falls at 30 % per second until it
  meets the level.
- The number in the slot stays the mean of all cores.
- On the wire, `cores` is a hex string with one byte for each core, in display order, and
  `nsuper` is how many of them are of the fastest type. Both are optional, so `v` stays 2. A
  sender without them gets the CPU curve, as before.

## Why

One number for 36 cores hides the case that the user cares about: a build that holds 4 cores at
100 % reads as 11 %. The bars show it at once.

The cores are grouped by type, not in kernel order: on the M5 Ultra the super cores are numbers
12–17 and 30–35, and the user found the real order too confusing. The bars have one height
because a benchmark of the two types gave performance ÷ super of 1.01–1.02 for integer and FMA
work and 0.65–0.75 only for a memory chase: a 0.9 weight does not show.

A hex string, not a JSON array, because the device's parser takes only a flat object of numbers.
It adds about 90 B to a reply, so `LINE_MAX` went from 256 to 512.

## Consequences

- ADR-0002 (columns are spans of time) no longer applies to the CPU row. The CPU row has no time
  history and no grid ticks.
- Two new levels, `Led` and `LedOff`, carry the segment gradient. The 1 bpp panel shows lit
  segments as ink and unlit ones as background.
- The device redraws only the segments that changed: all 612 every frame was too many rects.
- The sender reads the cluster types once, from `ioreg`. With no cluster types, the bars are in
  kernel order and all violet.
