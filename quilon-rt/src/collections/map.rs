// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! Native backing for Quilon's built-in `Map` type: a `std::collections::HashMap` wrapped
//! in a GC-allocated header. See `super` for the GC-visibility and immutability contract.

#![allow(clippy::not_unsafe_ptr_arg_deref)]

use super::common::{FixedState, QlKey, debug_check_user_key, gc_alloc};
use std::collections::HashMap;
use std::os::raw::c_void;

/// GC-managed native map header. See the module docs for the GC-visibility contract.
#[repr(C)]
struct QlMap {
    table: HashMap<QlKey, *const c_void, FixedState>,
    snapshot_a: *const u64,
    snapshot_b: *const u64,
    snapshot_values: *const *const c_void,
    len: i64,
}

/// Move `table` into a fresh GC-allocated header, building the ordered snapshot (which
/// also anchors every key's bytes and value box for the collector).
unsafe fn build_map(table: HashMap<QlKey, *const c_void, FixedState>) -> *mut QlMap {
    let n = table.len();
    let snapshot_a = unsafe { gc_alloc::<u64>(n) };
    let snapshot_b = unsafe { gc_alloc::<u64>(n) };
    let snapshot_values = unsafe { gc_alloc::<*const c_void>(n) };
    for (i, (key, value)) in table.iter().enumerate() {
        unsafe {
            *snapshot_a.add(i) = key.a;
            *snapshot_b.add(i) = key.b;
            *snapshot_values.add(i) = *value;
        }
    }
    let header = unsafe { gc_alloc::<QlMap>(1) };
    unsafe {
        std::ptr::write(
            header,
            QlMap {
                table,
                snapshot_a,
                snapshot_b,
                snapshot_values,
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
#[allow(clippy::too_many_arguments)]
pub extern "C" fn __map_set(
    map: *const c_void,
    tag: i64,
    a: i64,
    b: i64,
    hash_fn: *const c_void,
    eq_fn: *const c_void,
    value: *const c_void,
) -> *mut c_void {
    let map = map as *const QlMap;
    let mut table = unsafe { (*map).table.clone() };
    let key = QlKey::new(tag, a, b, hash_fn, eq_fn);
    debug_check_user_key(table.keys(), &key);
    table.insert(key, value);
    unsafe { build_map(table) as *mut c_void }
}

#[unsafe(no_mangle)]
pub extern "C" fn __map_remove(
    map: *const c_void,
    tag: i64,
    a: i64,
    b: i64,
    hash_fn: *const c_void,
    eq_fn: *const c_void,
) -> *mut c_void {
    let map = map as *const QlMap;
    let mut table = unsafe { (*map).table.clone() };
    table.remove(&QlKey::new(tag, a, b, hash_fn, eq_fn));
    unsafe { build_map(table) as *mut c_void }
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn __map_get(
    map: *const c_void,
    tag: i64,
    a: i64,
    b: i64,
    hash_fn: *const c_void,
    eq_fn: *const c_void,
    found_out: *mut i64,
) -> *const c_void {
    let map = map as *const QlMap;
    match unsafe { (*map).table.get(&QlKey::new(tag, a, b, hash_fn, eq_fn)) } {
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
pub extern "C" fn __map_has(
    map: *const c_void,
    tag: i64,
    a: i64,
    b: i64,
    hash_fn: *const c_void,
    eq_fn: *const c_void,
) -> i64 {
    let map = map as *const QlMap;
    unsafe { (*map).table.contains_key(&QlKey::new(tag, a, b, hash_fn, eq_fn)) as i64 }
}

#[unsafe(no_mangle)]
pub extern "C" fn __map_len(map: *const c_void) -> i64 {
    unsafe { (*(map as *const QlMap)).len }
}

#[unsafe(no_mangle)]
pub extern "C" fn __map_key_a(map: *const c_void, i: i64) -> i64 {
    unsafe { *(*(map as *const QlMap)).snapshot_a.add(i as usize) as i64 }
}

#[unsafe(no_mangle)]
pub extern "C" fn __map_key_b(map: *const c_void, i: i64) -> i64 {
    unsafe { *(*(map as *const QlMap)).snapshot_b.add(i as usize) as i64 }
}

#[unsafe(no_mangle)]
pub extern "C" fn __map_val(map: *const c_void, i: i64) -> *const c_void {
    unsafe { *(*(map as *const QlMap)).snapshot_values.add(i as usize) }
}
