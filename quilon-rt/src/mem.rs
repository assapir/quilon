// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! Internal runtime primitives with no `core.*` language home: allocation and the
//! Boehm-GC binding (`__alloc`, `__gc_init`), the shared `QlSlice` `{ ptr, len }`
//! ABI type and its `alloc_text` helper, the `format_num` render helper, and the
//! fail-loud `__index_fail` bounds-check primitive (checked `arr[i]` has no
//! `core.*` module, so it lives in this internal tier). This tier is where the
//! future fiber scheduler and reactor will also live.

use crate::report::{QlSite, fail_at};
use std::os::raw::c_void;

// Link the Boehm GC and tie it to these symbol references so the linker keeps
// libgc for every target (binary, tests, JIT harness) regardless of `--as-needed`
// ordering. libgc must be installed (`libgc-dev` / `gc`); CI installs it.
#[link(name = "gc")]
unsafe extern "C" {
    fn GC_malloc(size: usize) -> *mut c_void;
    fn GC_init();
    fn GC_allow_register_threads();
    fn GC_register_my_thread(sb: *const GcStackBase) -> i32;
    fn GC_unregister_my_thread() -> i32;
    fn GC_get_stack_base(sb: *mut GcStackBase) -> i32;
}

/// Boehm's description of a thread's stack extent, filled in by `GC_get_stack_base`.
#[repr(C)]
struct GcStackBase {
    mem_base: *mut c_void,
}

/// Initialize the garbage collector. Emitted as the first call in `main`.
#[unsafe(no_mangle)]
pub extern "C" fn __gc_init() {
    // Safe to call more than once; GC_init is idempotent.
    unsafe { GC_init() }
}

/// Prepare the collector for threads other than the one that initialized it, and
/// register the calling thread with it until the returned guard is dropped.
///
/// The collector stops the world by signalling the threads it knows, and it only knows
/// the thread that initialized it plus any it was told about. A thread it has not been
/// told about is not merely unscanned: when a collection happens the process aborts,
/// with a message that varies by timing — `Collecting from unknown thread`,
/// `pthread_kill failed at suspend`, `Signals delivery fails constantly`. A compiled
/// Quilon program never meets this, because it has one thread. A *host* that runs
/// Quilon code on more than one thread does, which is why the JIT calls this.
///
/// Initialization runs once however many threads arrive at it together: two threads
/// initializing at the same time abort with `Exclusion ranges overlap`.
pub fn register_thread() -> GcThread {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| unsafe {
        GC_init();
        GC_allow_register_threads();
    });

    let registered = unsafe {
        let mut base = GcStackBase {
            mem_base: std::ptr::null_mut(),
        };
        GC_get_stack_base(&mut base);
        // 0 says we registered it, 1 says it was already known — see `GcThread` for why
        // both count as ours to remove.
        matches!(GC_register_my_thread(&base), 0 | 1)
    };
    GcThread { registered }
}

/// Unregisters the thread when it is dropped.
///
/// Taking the thread back out matters as much as putting it in, because the collector's
/// knowledge outlives the thread: an entry left behind is a corpse every later collection
/// tries to stop. That includes the thread that initialized the collector, which is
/// already known without registering — so it is unregistered here too.
pub struct GcThread {
    registered: bool,
}

impl Drop for GcThread {
    fn drop(&mut self) {
        if self.registered {
            unsafe { GC_unregister_my_thread() };
        }
    }
}

/// Allocate `size` bytes of GC-managed, zeroed-on-demand memory.
///
/// Returns a pointer the collector tracks; callers never free it. A non-positive
/// size yields a 1-byte allocation so the result is always a valid, unique-ish
/// pointer.
#[unsafe(no_mangle)]
pub extern "C" fn __alloc(size: i64) -> *mut c_void {
    let n = if size <= 0 { 1 } else { size as usize };
    unsafe { GC_malloc(n) }
}

/// Report an invalid array index — out of bounds, negative, or NaN — at the indexing
/// expression that asked for it, and terminate with exit status 1: the fail-loud contract of
/// checked `arr[i]` indexing.
///
/// `index` is the ORIGINAL f64 the program computed (pre-truncation), so the message shows
/// what the user actually asked for; `size` is the array's element count; `site` is the
/// `arr[i]` expression's own location, which the report frames the same way a failing
/// assertion does. Codegen calls this from the invalid branch of every `arr[i]` bounds check.
///
/// # Safety contract (upheld by the compiler)
/// `site` is null or points to a valid [`QlSite`].
#[unsafe(no_mangle)]
pub extern "C" fn __index_fail(index: f64, size: i64, site: *const QlSite) -> ! {
    fail_at(
        site,
        &format!(
            "index {} out of bounds for an array of size {}",
            format_num(index),
            size
        ),
        1,
    )
}

/// A Quilon `Text` value (also the representation of an array): `{ ptr data, i64 len }`,
/// matching the code generator's `ptr_len_struct_type` (`{ i8*, i64 }`). For a `Text`,
/// `data` points to `len` UTF-8 bytes; for an array, `data` points to `len` contiguous
/// element-representation values and `len` is the element count. `#[repr(C)]` so the
/// field offsets (ptr at 0, i64 at 8) match what LLVM emits.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct QlSlice {
    pub(crate) data: *const c_void,
    pub(crate) len: i64,
}

impl QlSlice {
    /// The empty slice (`{ null, 0 }`) — a zero-length `Text`/array. Returned when there
    /// is nothing to build (null/empty `argv`/`envp`).
    pub(crate) fn empty() -> QlSlice {
        QlSlice {
            data: std::ptr::null(),
            len: 0,
        }
    }

    /// This slice's bytes decoded as `Text` — empty when it carries none (a null pointer or
    /// a non-positive length). Reads it exactly as the `Text` intrinsics do.
    ///
    /// # Safety contract (upheld by the compiler)
    /// A non-null `data` points to at least `len` readable bytes.
    pub(crate) fn as_text(&self) -> std::borrow::Cow<'_, str> {
        crate::text::text_str(self.data as *const u8, self.len)
    }

    /// Whether this slice carries no bytes.
    pub(crate) fn is_empty(&self) -> bool {
        self.data.is_null() || self.len <= 0
    }
}

/// GC-allocate a `Text` whose bytes are a copy of `bytes`. The copy is owned by the GC
/// (so it outlives the C `argv`/`envp` buffers, which the program may not keep), and is
/// NUL-terminated past `len` so `print`/`eprint` (which expect a C string) work too.
pub(crate) fn alloc_text(bytes: &[u8]) -> QlSlice {
    let len = bytes.len();
    // +1 for a trailing NUL so the buffer doubles as a C string for `print`.
    let buf = __alloc(len as i64 + 1) as *mut u8;
    if !buf.is_null() && len > 0 {
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, len) };
    }
    QlSlice {
        data: buf as *const c_void,
        len: len as i64,
    }
}

/// Render an `f64` the way Quilon shows a `Num`: whole values without a fractional part
/// (`5`, not `5.0`), everything else in shortest round-trip form. Shared by `__num_to_text`
/// and the `__index_fail` diagnostic.
pub(crate) fn format_num(x: f64) -> String {
    if x.is_finite() && x.fract() == 0.0 && x.abs() < 1e15 {
        format!("{}", x as i64)
    } else {
        format!("{}", x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::GC_LOCK;

    #[test]
    fn format_num_drops_trailing_zeros_for_whole_values() {
        assert_eq!(format_num(3.0), "3");
        assert_eq!(format_num(120.0), "120");
        assert_eq!(format_num(3.5), "3.5");
    }

    #[test]
    fn alloc_returns_usable_memory() {
        let _g = GC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        __gc_init();
        let p = __alloc(16) as *mut u8;
        assert!(!p.is_null());
        unsafe {
            std::ptr::write_bytes(p, 0xAB, 16);
            assert_eq!(*p, 0xAB);
        }
    }
}
