// Runtime module for Quilon.
//
// `intrinsics` holds the C-ABI symbols linked into compiled programs (allocation
// via the Boehm GC, grapheme counting, basic output). `io` and `parallel` remain
// placeholders for the planned non-blocking-IO / implicit-parallelism runtime.

pub mod intrinsics;
pub mod io;
pub mod parallel;
