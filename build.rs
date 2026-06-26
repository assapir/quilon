// Build script for Quilon.
//
// The Boehm GC (libgc) is linked via a `#[link(name = "gc")]` extern block in
// `src/runtime/intrinsics.rs` rather than here: attaching the link to the actual
// `GC_malloc`/`GC_init` symbol references keeps the linker from dropping libgc
// under `--as-needed` (which a bare `cargo:rustc-link-lib=gc` here is subject to,
// depending on link order). This script only declares its own rerun trigger.
//
// libgc must be installed to build/run Quilon (e.g. `libgc-dev` on Debian/Ubuntu,
// `gc` on Arch). CI installs it explicitly.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
}
