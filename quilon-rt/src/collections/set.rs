// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! Native backing for Quilon's built-in `Set` type: a `std::collections::HashSet` wrapped
//! in a GC-allocated header, plus the set-algebra operators. See `super` for the
//! GC-visibility and immutability contract.

#![allow(clippy::not_unsafe_ptr_arg_deref)]

use super::common::{FixedState, QlKey, debug_check_user_key};
use crate::mem::alloc_slots;
use std::collections::HashSet;
use std::os::raw::c_void;

/// GC-managed native set header (element analogue of the map header, with no values).
#[repr(C)]
struct QlSet {
    table: HashSet<QlKey, FixedState>,
    snapshot_a: *const u64,
    snapshot_b: *const u64,
    len: i64,
}

unsafe fn build_set(table: HashSet<QlKey, FixedState>) -> *mut QlSet {
    let n = table.len();
    let snapshot_a = alloc_slots::<u64>(n);
    let snapshot_b = alloc_slots::<u64>(n);
    for (i, key) in table.iter().enumerate() {
        unsafe {
            *snapshot_a.add(i) = key.a;
            *snapshot_b.add(i) = key.b;
        }
    }
    let header = alloc_slots::<QlSet>(1);
    unsafe {
        std::ptr::write(
            header,
            QlSet {
                table,
                snapshot_a,
                snapshot_b,
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
pub extern "C" fn __set_add(
    set: *const c_void,
    tag: i64,
    a: i64,
    b: i64,
    hash_fn: *const c_void,
    eq_fn: *const c_void,
) -> *mut c_void {
    let set = set as *const QlSet;
    let mut table = unsafe { (*set).table.clone() };
    let key = QlKey::new(tag, a, b, hash_fn, eq_fn);
    debug_check_user_key(table.iter(), &key);
    table.insert(key);
    unsafe { build_set(table) as *mut c_void }
}

#[unsafe(no_mangle)]
pub extern "C" fn __set_remove(
    set: *const c_void,
    tag: i64,
    a: i64,
    b: i64,
    hash_fn: *const c_void,
    eq_fn: *const c_void,
) -> *mut c_void {
    let set = set as *const QlSet;
    let mut table = unsafe { (*set).table.clone() };
    table.remove(&QlKey::new(tag, a, b, hash_fn, eq_fn));
    unsafe { build_set(table) as *mut c_void }
}

#[unsafe(no_mangle)]
pub extern "C" fn __set_has(
    set: *const c_void,
    tag: i64,
    a: i64,
    b: i64,
    hash_fn: *const c_void,
    eq_fn: *const c_void,
) -> i64 {
    let set = set as *const QlSet;
    unsafe {
        (*set)
            .table
            .contains(&QlKey::new(tag, a, b, hash_fn, eq_fn)) as i64
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn __set_len(set: *const c_void) -> i64 {
    unsafe { (*(set as *const QlSet)).len }
}

#[unsafe(no_mangle)]
pub extern "C" fn __set_item_a(set: *const c_void, i: i64) -> i64 {
    unsafe { *(*(set as *const QlSet)).snapshot_a.add(i as usize) as i64 }
}

#[unsafe(no_mangle)]
pub extern "C" fn __set_item_b(set: *const c_void, i: i64) -> i64 {
    unsafe { *(*(set as *const QlSet)).snapshot_b.add(i as usize) as i64 }
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

/// A new set of `left`'s elements filtered by membership in `right`: `keep_present` true
/// keeps those present in `right` (intersection), false those absent (difference).
unsafe fn set_filter(left: *const c_void, right: *const c_void, keep_present: bool) -> *mut c_void {
    let (left, right) = (left as *const QlSet, right as *const QlSet);
    let mut table = HashSet::with_hasher(FixedState);
    for key in unsafe { (*left).table.iter() } {
        if unsafe { (*right).table.contains(key) } == keep_present {
            table.insert(*key);
        }
    }
    unsafe { build_set(table) as *mut c_void }
}

#[unsafe(no_mangle)]
pub extern "C" fn __set_diff(left: *const c_void, right: *const c_void) -> *mut c_void {
    unsafe { set_filter(left, right, false) }
}

#[unsafe(no_mangle)]
pub extern "C" fn __set_intersect(left: *const c_void, right: *const c_void) -> *mut c_void {
    unsafe { set_filter(left, right, true) }
}
