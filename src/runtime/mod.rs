// Runtime module for Quilon.
//
// The C-ABI runtime intrinsics (allocation via the Boehm GC, grapheme counting,
// basic output) now live in the separate `quilon-rt` crate so they can be packaged
// as `libquilon_rt.a` and linked into ahead-of-time-compiled native binaries. They
// are re-exported here as `runtime::intrinsics` so the in-process JIT keeps mapping
// the same `#[no_mangle]` symbols. `io` and `parallel` remain placeholders for the
// planned non-blocking-IO / implicit-parallelism runtime.

pub mod io;
pub mod parallel;

// Re-export so existing `crate::runtime::intrinsics::*` references keep working.
pub use quilon_rt as intrinsics;
