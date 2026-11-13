# CPU comes from Mach ticks; macmon stays for GPU only

The sender reads CPU usage natively from the Mach tick counters at the frame rate and
keeps the `macmon` subprocess for the GPU number alone. CPU therefore means
`busy / (busy + idle)` across all cores — the definition Activity Monitor uses — instead of
macmon's `cpu_active_ratio`.

## Why

Measured on an M5 Ultra (36 cores): the Mach call costs 1.3 µs and its ticks run at about 3.5 kHz,
so a 33 ms frame window holds roughly 118 ticks. `macmon` tops out at 8.5–9.3 lines per second.
Taking CPU from macmon inherits that ceiling for no reason.

`macmon` stays for GPU because the public registry mirror is worse: `IOAccelerator`'s
`PerformanceStatistics` reads in 36 µs and can be polled at ~28 kHz, but its `Device Utilization %`
changed 4 times in 12 seconds during GPU work, while `gpu_active_ratio` from macmon changed about
6.5 times per second. macmon reads the private `IOReport` API; the registry key is a stale mirror.
Reimplementing `IOReport` in Rust means owning the fragile part ourselves, for no gain.

**Correction, 2026-11-12.** The host-wide call, `host_statistics(HOST_CPU_LOAD_INFO)`, moved only
about once a second on macOS 27, so it cannot feed a per-frame number. The per-core call,
`host_processor_info(PROCESSOR_CPU_LOAD_INFO)`, moves every few milliseconds and costs about 22 µs
from Python. The sender sums the per-core ticks. The decision and the CPU definition do not change;
only the call does. Evidence: `docs/device-v2.md`, Mac-side sources.

## Consequences

- **The CPU number changes definition.** Measured side by side: native 1.4–4.8 %, macmon 1.5–3.0 %.
  Close, not equal. After this lands, the number is not comparable with old screenshots.
- All three metrics now match Activity Monitor, which v1 already promised for memory only.
- `macmon` becomes optional: with it absent the device still shows CPU and memory, and the GPU row
  reads as unavailable. That closes the "missing macmon fakes live values" half of issue #1.
- `macmon`'s restart-on-death logic stays, but it can no longer freeze two thirds of the panel.
- Three freshness classes on one wire: CPU per frame, memory ~10 Hz, GPU ~6.5 Hz. Each sample
  carries its own arrival stamp.
