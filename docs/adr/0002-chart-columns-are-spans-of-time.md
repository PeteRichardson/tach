# Chart columns are spans of time, not slots for samples

A chart column is a fixed span of time, and the device places each sample by its timestamp.
Previously one column was one sample, so the chart could only move once per message — two
pixels per second in two steps — and motion smoothness was tied to the sample rate. With
time-positioned columns the curve slides continuously at the frame rate while samples keep
arriving twice each second, and nothing is invented: each column still shows the true lowest
and highest sample within its span.

## Consequences

The device keeps the sample history and recomputes the columns on every frame, so it needs
RAM for that history. It does not need to transfer a whole frame each time: what changes is
the shift plus the newest columns. That distinction is what makes the model workable on the
RA8875, whose 12 MHz SPI link cannot carry a whole 480 × 272 frame 30 times each second.
This decision supersedes the column-per-sample wording in design.md sections 5 and 6.2, and
it makes the `macmon` sample-rate ceiling irrelevant to motion.
