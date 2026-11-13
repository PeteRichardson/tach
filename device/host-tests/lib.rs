//! Host-side tests for the firmware modules that do not touch hardware. The firmware crate builds
//! for thumbv8m only, so its `#[cfg(test)]` modules run here instead, compiled from the same files.
//!
//! Run from the repository root, so that device/.cargo/config.toml does not force the RP2350
//! target:
//!
//! ```text
//! cargo test --manifest-path device/host-tests/Cargo.toml
//! ```

#![allow(dead_code)]

#[path = "../src/chart.rs"]
mod chart;
#[path = "../src/framebuf.rs"]
mod framebuf;
#[path = "../src/layout.rs"]
mod layout;
#[path = "../src/ops.rs"]
mod ops;
#[path = "../src/text.rs"]
mod text;
#[path = "../src/wire.rs"]
mod wire;
