# The firmware sends drawing operations, not frames

The firmware draws with panel operations — `Scroll`, `Line`, `Rectangle`, `Ellipse`, `Glyph`,
`Clear`, `Brightness` — and each panel backend maps them onto what its controller does. It never
builds and pushes a whole frame on the target hardware. The 2.7" SHARP scaffold has no VRAM, so
its backend shims the operations into writes on a frame buffer in MCU RAM plus one flush per
frame.

## Why not frames

Full-frame RGB565 at 480×272 and 30 fps is 7.8 Mbyte/s. The RA8875 SPI reference driver runs at
12 MHz, about 1.5 Mbyte/s, so a frame-push design is impossible on this link. The controller has
768 KB of VRAM and hardware shapes and scroll, so a chart tick is one scroll command plus a few
short draw commands, and the pixels never cross USB.

## Consequences

- The RA8875 backend is thin: command encode, register write, done.
- The SHARP backend is a shim: same operations, whole-frame flush. It is temporary and can be
  deleted when the RA8875 arrives.
- Geometry must travel with the operation, which is why layout derives from panel dimensions.
- There is no single image of the screen in RAM on the target hardware, so screenshots and visual
  regression tests need the scaffold or the serial protocol, not the panel.
- Hardware scroll semantics on the RA8875 (wrap, vacated area) must be verified before the travel
  path is built.
