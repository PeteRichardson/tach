# The device pulls one sample per frame

The device asks the Mac for a sample once per display frame, and the Mac answers with every metric
it has plus the arrival time of each. The Mac no longer pushes on its own timer.

## Why

v1 has the Mac push on a timer and the device redraw when a line arrives. That couples three
things that should be independent — the measurement rate, the wire rate and the frame rate — and it
is why the display refreshes at the sample rate. It also leaves the rate unwritten on the wire
(issue #10) and makes the frame grid depend on when bytes happen to land.

With a pull, the newest sample is always exactly where the frame needs it, the frame grid is
perfectly even because the device owns the clock, and the two sides have no rate to agree on. The
first reply of a session also carries the sample interval and the sender version, which is what
issue #38 needs.

## Consequences

- The device needs an outbound path and the sender an inbound path. `tach-sender.py` is one-way
  today, so this is work on both sides.
- Staleness becomes trivial to state: no reply within N frames means the Mac stopped answering, so
  blank the panel (issue #1, issue #27).
- The wire carries per-metric arrival stamps, because the three metrics have different freshness:
  CPU ticks change every poll, memory about 10 times a second, GPU about 6.5 (see
  `docs/device-v2.md`, Mac-side sources).
- Link round trip is included in the frame budget. Over USB CDC full speed this is sub-millisecond;
  measure it on the scaffold.
- If a reply is late, the frame draws with the last known samples and the curve keeps travelling by
  time. Motion does not depend on the wire.
