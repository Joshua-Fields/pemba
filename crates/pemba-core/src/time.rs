//! The clock behind check timings and trace durations.
//!
//! `std::time::Instant::now` traps on `wasm32-unknown-unknown`, which the playground build
//! targets (ADR-016, `docs/11-playground.md` §1.2); `web-time` reads `performance.now()`
//! there. Other targets keep `std::time` and gain no dependency.

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use std::time::Instant;
#[cfg(target_arch = "wasm32")]
pub(crate) use web_time::Instant;
