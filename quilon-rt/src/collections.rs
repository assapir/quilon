// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! Native backing for Quilon's built-in `Map` and `Set` collection types.
//!
//! Each collection is a plain `std::collections::HashMap`/`HashSet` (SwissTable under the
//! hood) wrapped in a GC-allocated header, with a **fixed-seed** `BuildHasher` so
//! iteration is stable run-to-run. Iteration ORDER is unspecified by contract (it is the
//! table's hash order, NOT insertion order); the fixed seed only makes it reproducible so
//! example-asserts don't flake.
//!
//! Immutability: every mutator (`__map_set`, `__set_add`, the set-algebra ops) CLONES the
//! table and returns a NEW header — the receiver is never touched.
//!
//! GC visibility: the `std` table's own buffer comes from the global allocator and is
//! invisible to the conservative Boehm collector, but that is safe here because the header
//! also holds GC-allocated *snapshot* arrays mirroring every key word and value pointer.
//! The collector scans those (the header is a scanned GC object) and so keeps every Text
//! key's bytes and every boxed value alive; the table's buffer is never freed because the
//! header is never dropped. The snapshots double as the ordered index the compiler reads
//! for `keys`/`values`/`items`/`each`.
//!
//! ABI: keys/elements arrive as the triple `(tag, a, b)` of `i64`s — `tag` = 0 Num / 1
//! Text / 2 Bool; `a`/`b` carry an f64's bits, a Bool's 0/1, or a Text's data pointer +
//! byte length. Values are opaque GC-box pointers the compiler loads back at their type.

#![allow(clippy::not_unsafe_ptr_arg_deref)]

use crate::io::write_to_fd;
use crate::mem::{__alloc, format_num};
use crate::process::__exit;
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, Hash, Hasher};
use std::os::raw::c_void;

// Key-kind tags shared with the compiler ABI. Num (0) needs no named constant here — it
// is the implicit default in `eq`/`hash`/`key_desc` (anything that is not Text or Bool).
const TAG_TEXT: u8 = 1;
const TAG_BOOL: u8 = 2;

/// A `BuildHasher` with a fixed seed: `DefaultHasher::new()` uses constant SipHash keys
/// (unlike `RandomState`, which randomizes per process), so a map/set iterates the same
/// way on every run — reproducible, though the order is still unspecified by contract.
#[derive(Clone, Default)]
struct FixedState;

impl BuildHasher for FixedState {
    type Hasher = std::collections::hash_map::DefaultHasher;
    fn build_hasher(&self) -> Self::Hasher {
        std::collections::hash_map::DefaultHasher::new()
    }
}

/// A hashable key from the compiler ABI. `Text` keys carry a `(pointer, length)` into GC
/// memory and hash/compare BY CONTENT, consistent with Quilon's value `==` on `Text`;
/// `Num`/`Bool` keys hash by their word bits.
#[derive(Clone, Copy)]
struct QlKey {
    tag: u8,
    a: u64,
    b: u64,
}

impl QlKey {
    fn new(tag: i64, a: i64, b: i64) -> QlKey {
        QlKey {
            tag: tag as u8,
            a: a as u64,
            b: b as u64,
        }
    }

    /// The bytes a `Text` key points at (empty for a null/zero-length key).
    unsafe fn text_bytes(&self) -> &'static [u8] {
        if self.a == 0 || self.b == 0 {
            return &[];
        }
        unsafe { std::slice::from_raw_parts(self.a as *const u8, self.b as usize) }
    }
}

impl PartialEq for QlKey {
    fn eq(&self, other: &Self) -> bool {
        if self.tag != other.tag {
            return false;
        }
        if self.tag == TAG_TEXT {
            unsafe { self.text_bytes() == other.text_bytes() }
        } else {
            self.a == other.a
        }
    }
}

impl Eq for QlKey {}

impl Hash for QlKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u8(self.tag);
        if self.tag == TAG_TEXT {
            unsafe { state.write(self.text_bytes()) };
        } else {
            state.write_u64(self.a);
        }
    }
}

/// GC-allocate space for `count` (at least one) values of type `T`, zeroed.
unsafe fn gc_alloc<T>(count: usize) -> *mut T {
    let bytes = std::mem::size_of::<T>() * count.max(1);
    __alloc(bytes as i64) as *mut T
}

/// Render a key for a diagnostic (the fail-loud `m[k]` message).
fn key_desc(key: &QlKey) -> String {
    match key.tag {
        TAG_TEXT => format!("\"{}\"", String::from_utf8_lossy(unsafe { key.text_bytes() })),
        TAG_BOOL => if key.a == 0 { "false" } else { "true" }.to_string(),
        _ => format_num(f64::from_bits(key.a)),
    }
}

// ---- Map ------------------------------------------------------------------

/// GC-managed native map header. See the module docs for the GC-visibility contract.
#[repr(C)]
struct QlMap {
    table: HashMap<QlKey, *const c_void, FixedState>,
    snap_a: *const u64,
    snap_b: *const u64,
    snap_v: *const *const c_void,
    len: i64,
}

/// Move `table` into a fresh GC-allocated header, building the ordered snapshot (which
/// also anchors every key's bytes and value box for the collector).
unsafe fn build_map(table: HashMap<QlKey, *const c_void, FixedState>) -> *mut QlMap {
    let n = table.len();
    let snap_a = unsafe { gc_alloc::<u64>(n) };
    let snap_b = unsafe { gc_alloc::<u64>(n) };
    let snap_v = unsafe { gc_alloc::<*const c_void>(n) };
    for (i, (key, value)) in table.iter().enumerate() {
        unsafe {
            *snap_a.add(i) = key.a;
            *snap_b.add(i) = key.b;
            *snap_v.add(i) = *value;
        }
    }
    let header = unsafe { gc_alloc::<QlMap>(1) };
    unsafe {
        std::ptr::write(
            header,
            QlMap {
                table,
                snap_a,
                snap_b,
                snap_v,
                len: n as i64,
            },
        )
    };
    header
}

#[unsafe(no_mangle)]
pub extern "C" fn __map_new() -> *mut c_void {
    unsafe { build_map(HashMap::with_hasher(FixedState)) as *mut c_void }
}

#[unsafe(no_mangle)]
pub extern "C" fn __map_set(
    map: *const c_void,
    tag: i64,
    a: i64,
    b: i64,
    value: *const c_void,
) -> *mut c_void {
    let map = map as *const QlMap;
    let mut table = unsafe { (*map).table.clone() };
    table.insert(QlKey::new(tag, a, b), value);
    unsafe { build_map(table) as *mut c_void }
}

#[unsafe(no_mangle)]
pub extern "C" fn __map_get(
    map: *const c_void,
    tag: i64,
    a: i64,
    b: i64,
    found_out: *mut i64,
) -> *const c_void {
    let map = map as *const QlMap;
    match unsafe { (*map).table.get(&QlKey::new(tag, a, b)) } {
        Some(value) => {
            unsafe { *found_out = 1 };
            *value
        }
        None => {
            unsafe { *found_out = 0 };
            std::ptr::null()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn __map_index(map: *const c_void, tag: i64, a: i64, b: i64) -> *const c_void {
    let map = map as *const QlMap;
    let key = QlKey::new(tag, a, b);
    match unsafe { (*map).table.get(&key) } {
        Some(value) => *value,
        None => {
            let msg = format!("runtime error: map key {} not found\n", key_desc(&key));
            write_to_fd(2, msg.as_bytes());
            __exit(1)
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn __map_has(map: *const c_void, tag: i64, a: i64, b: i64) -> i64 {
    let map = map as *const QlMap;
    unsafe { (*map).table.contains_key(&QlKey::new(tag, a, b)) as i64 }
}

#[unsafe(no_mangle)]
pub extern "C" fn __map_len(map: *const c_void) -> i64 {
    unsafe { (*(map as *const QlMap)).len }
}

#[unsafe(no_mangle)]
pub extern "C" fn __map_key_a(map: *const c_void, i: i64) -> i64 {
    unsafe { *(*(map as *const QlMap)).snap_a.add(i as usize) as i64 }
}

#[unsafe(no_mangle)]
pub extern "C" fn __map_key_b(map: *const c_void, i: i64) -> i64 {
    unsafe { *(*(map as *const QlMap)).snap_b.add(i as usize) as i64 }
}

#[unsafe(no_mangle)]
pub extern "C" fn __map_val(map: *const c_void, i: i64) -> *const c_void {
    unsafe { *(*(map as *const QlMap)).snap_v.add(i as usize) }
}

// ---- Set ------------------------------------------------------------------

/// GC-managed native set header (element analogue of [`QlMap`], with no values).
#[repr(C)]
struct QlSet {
    table: HashSet<QlKey, FixedState>,
    snap_a: *const u64,
    snap_b: *const u64,
    len: i64,
}

unsafe fn build_set(table: HashSet<QlKey, FixedState>) -> *mut QlSet {
    let n = table.len();
    let snap_a = unsafe { gc_alloc::<u64>(n) };
    let snap_b = unsafe { gc_alloc::<u64>(n) };
    for (i, key) in table.iter().enumerate() {
        unsafe {
            *snap_a.add(i) = key.a;
            *snap_b.add(i) = key.b;
        }
    }
    let header = unsafe { gc_alloc::<QlSet>(1) };
    unsafe {
        std::ptr::write(
            header,
            QlSet {
                table,
                snap_a,
                snap_b,
                len: n as i64,
            },
        )
    };
    header
}

#[unsafe(no_mangle)]
pub extern "C" fn __set_new() -> *mut c_void {
    unsafe { build_set(HashSet::with_hasher(FixedState)) as *mut c_void }
}

#[unsafe(no_mangle)]
pub extern "C" fn __set_add(set: *const c_void, tag: i64, a: i64, b: i64) -> *mut c_void {
    let set = set as *const QlSet;
    let mut table = unsafe { (*set).table.clone() };
    table.insert(QlKey::new(tag, a, b));
    unsafe { build_set(table) as *mut c_void }
}

#[unsafe(no_mangle)]
pub extern "C" fn __set_has(set: *const c_void, tag: i64, a: i64, b: i64) -> i64 {
    let set = set as *const QlSet;
    unsafe { (*set).table.contains(&QlKey::new(tag, a, b)) as i64 }
}

#[unsafe(no_mangle)]
pub extern "C" fn __set_len(set: *const c_void) -> i64 {
    unsafe { (*(set as *const QlSet)).len }
}

#[unsafe(no_mangle)]
pub extern "C" fn __set_item_a(set: *const c_void, i: i64) -> i64 {
    unsafe { *(*(set as *const QlSet)).snap_a.add(i as usize) as i64 }
}

#[unsafe(no_mangle)]
pub extern "C" fn __set_item_b(set: *const c_void, i: i64) -> i64 {
    unsafe { *(*(set as *const QlSet)).snap_b.add(i as usize) as i64 }
}

#[unsafe(no_mangle)]
pub extern "C" fn __set_union(left: *const c_void, right: *const c_void) -> *mut c_void {
    let (left, right) = (left as *const QlSet, right as *const QlSet);
    let mut table = unsafe { (*left).table.clone() };
    for key in unsafe { (*right).table.iter() } {
        table.insert(*key);
    }
    unsafe { build_set(table) as *mut c_void }
}

#[unsafe(no_mangle)]
pub extern "C" fn __set_diff(left: *const c_void, right: *const c_void) -> *mut c_void {
    let (left, right) = (left as *const QlSet, right as *const QlSet);
    let mut table = HashSet::with_hasher(FixedState);
    for key in unsafe { (*left).table.iter() } {
        if unsafe { !(*right).table.contains(key) } {
            table.insert(*key);
        }
    }
    unsafe { build_set(table) as *mut c_void }
}

#[unsafe(no_mangle)]
pub extern "C" fn __set_intersect(left: *const c_void, right: *const c_void) -> *mut c_void {
    let (left, right) = (left as *const QlSet, right as *const QlSet);
    let mut table = HashSet::with_hasher(FixedState);
    for key in unsafe { (*left).table.iter() } {
        if unsafe { (*right).table.contains(key) } {
            table.insert(*key);
        }
    }
    unsafe { build_set(table) as *mut c_void }
}
