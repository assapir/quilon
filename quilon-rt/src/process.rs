// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! General process / runtime-lifecycle primitives: the process-exit primitive
//! (`__exit`, and where the future `__panic` will go) and the entry-point
//! startup conversions that turn the C `argv`/`envp` `main` receives into a Quilon
//! `[]Text` (args) and a `[|Text => Text|]` Map (env).

use crate::mem::{__alloc, QlSlice, alloc_text};
use std::ffi::CStr;
use std::os::raw::{c_char, c_int, c_void};

/// Terminate the running program with exit status `code`.
///
/// Quilon cannot yet exit/abort mid-program in-language, so the exit primitive lives
/// here as a generic `__exit(code)`. It backs both a failing assertion (exit 101 — the
/// Rust-panic convention, deliberately distinct from the small result codes examples
/// use as their normal exit status) and the runtime's own fail-loud paths. Codegen lowers a `__exit(n)` call to a call of this symbol;
/// see `CodeGenerator::generate_exit`.
///
/// Never returns. Uses libc `exit(3)` directly rather than `std::process::exit` for
/// the same reason `write_to_fd` uses raw `write(2)`: an AOT-linked native binary
/// enters through the LLVM-generated C `main`, so the Rust std runtime is never
/// initialized.
#[unsafe(no_mangle)]
pub extern "C" fn __exit(code: c_int) -> ! {
    // SAFETY: libc `exit` is always available in a linked C runtime; it terminates
    // the process and never returns.
    unsafe extern "C" {
        fn exit(code: c_int) -> !;
    }
    unsafe { exit(code) }
}

/// Build a Quilon `[]Text` from the C `argc`/`argv` the program's `main` received: one
/// `Text` per argument (including `argv[0]`, the program name), in order. Backs an `^`
/// entry point that declares `args :: []Text`.
///
/// Returns the array as a `{ ptr, i64 }` `QlSlice` whose `data` points to `argc`
/// contiguous `Text` structs — exactly the layout codegen loads for a `[]Text` value.
///
/// # Safety contract (upheld by the C runtime / `main`)
/// `argv` is null, or points to `argc` valid NUL-terminated C strings (the standard
/// `main` contract); a non-positive `argc` yields an empty array.
// Exported C-ABI symbol called from generated code; a safe Rust signature is
// intentional (the contract is upheld by the compiler emitting the call).
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __argv_to_text_array(argc: i64, argv: *const *const c_char) -> QlSlice {
    if argv.is_null() || argc <= 0 {
        return QlSlice::empty();
    }
    let n = argc as usize;
    // Allocate the backing array of `n` Text structs (GC-owned).
    let elems = __alloc((n * std::mem::size_of::<QlSlice>()) as i64) as *mut QlSlice;
    for i in 0..n {
        // SAFETY: `argv[0..argc]` are valid C strings per the `main` contract.
        let cstr = unsafe { *argv.add(i) };
        let bytes = cstr_to_str_bytes(cstr);
        let text = alloc_text(&bytes);
        unsafe { std::ptr::write(elems.add(i), text) };
    }
    QlSlice {
        data: elems as *const c_void,
        len: argc,
    }
}

/// Build a Quilon `[|Text => Text|]` Map from the C `envp` the program's `main` received:
/// one entry per environment variable, its name and value split on the FIRST `=`. An entry
/// with no `=` maps the whole string to `""` (empty value). Backs an `^` entry point that
/// declares `env :: [|Text => Text|]`, in the same native Map representation `[|…|]`
/// literals lower to (so `env.get("KEY")` works directly).
///
/// `envp` is the conventional NULL-terminated array of `key=value` C strings.
///
/// # Safety contract (upheld by the C runtime / `main`)
/// `envp` is null, or points to a NULL-terminated array of valid NUL-terminated C
/// strings (the standard `main`/`environ` contract).
// Exported C-ABI symbol called from generated code; a safe Rust signature is
// intentional (the contract is upheld by the compiler emitting the call).
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn __envp_to_map(envp: *const *const c_char) -> *mut c_void {
    // Borrow each entry's bytes straight from the C `envp` (valid for the whole call) and
    // split on the first '='; `build_text_map` copies every key/value into GC memory before
    // it returns, so nothing needs to be owned here.
    let mut entries: Vec<(&[u8], &[u8])> = Vec::new();
    if !envp.is_null() {
        let mut i = 0usize;
        while !unsafe { *envp.add(i) }.is_null() {
            // SAFETY: entry `i` is non-null per the loop condition.
            let bytes = unsafe { CStr::from_ptr(*envp.add(i)) }.to_bytes();
            entries.push(match bytes.iter().position(|&b| b == b'=') {
                Some(eq) => (&bytes[..eq], &bytes[eq + 1..]),
                None => (bytes, &[] as &[u8]),
            });
            i += 1;
        }
    }
    crate::collections::build_text_map(entries.into_iter())
}

/// The bytes of a NUL-terminated C string (empty for null). Used to copy `argv`/`envp`
/// entries into GC-owned `Text`s.
fn cstr_to_str_bytes(ptr: *const c_char) -> Vec<u8> {
    if ptr.is_null() {
        return Vec::new();
    }
    unsafe { CStr::from_ptr(ptr) }.to_bytes().to_vec()
}
