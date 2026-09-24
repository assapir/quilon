// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! Native backing for Quilon's built-in `Set` type: a `std::collections::HashSet` wrapped
//! in a GC-allocated header, plus the set-algebra operators. See `super` for the
//! GC-visibility and immutability contract.

#![allow(clippy::not_unsafe_ptr_arg_deref)]

use super::common::{FixedState, QnKey, debug_check_user_key};
use crate::mem::alloc_slots;
use std::collections::HashSet;
use std::os::raw::c_void;

/// One snapshot slot: an element's identity pair, kept in one GC-allocated array (see
/// [`refresh_snapshot`]) instead of two arrays that must be rebuilt, and stay the same
/// length, in lockstep.
#[repr(C)]
struct QnSetEntry {
    key_a: u64,
    key_b: u64,
}

/// GC-managed native set header (element analogue of the map header, with no values).
#[repr(C)]
struct QnSet {
    table: HashSet<QnKey, FixedState>,
    snapshot: *const QnSetEntry,
    len: i64,
}

unsafe fn build_set(table: HashSet<QnKey, FixedState>) -> *mut QnSet {
    let header = alloc_slots::<QnSet>(1);
    unsafe {
        std::ptr::write(
            header,
            QnSet {
                table,
                snapshot: std::ptr::null(),
                len: 0,
            },
        );
        refresh_snapshot(header);
    }
    header
}

/// Rebuild `header`'s ordered snapshot array and `len` from its current `table`. Called
/// after every in-place mutation (`__set_add`/`__set_remove`).
unsafe fn refresh_snapshot(header: *mut QnSet) {
    let table = unsafe { &(*header).table };
    let n = table.len();
    let snapshot = alloc_slots::<QnSetEntry>(n);
    for (i, key) in table.iter().enumerate() {
        unsafe {
            std::ptr::write(
                snapshot.add(i),
                QnSetEntry {
                    key_a: key.a,
                    key_b: key.b,
                },
            );
        }
    }
    unsafe {
        (*header).snapshot = snapshot;
        (*header).len = n as i64;
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn __set_new() -> *mut c_void {
    unsafe { build_set(HashSet::with_hasher(FixedState)) as *mut c_void }
}

/// Insert `key` into `set` IN PLACE and return the same header: `add` is a mutator (see
/// the module docs), not a constructor.
#[unsafe(no_mangle)]
pub extern "C" fn __set_add(
    set: *const c_void,
    tag: i64,
    a: i64,
    b: i64,
    hash_fn: *const c_void,
    eq_fn: *const c_void,
) -> *mut c_void {
    let header = set as *mut QnSet;
    let key = QnKey::new(tag, a, b, hash_fn, eq_fn);
    unsafe {
        debug_check_user_key((*header).table.iter(), &key);
        (*header).table.insert(key);
        refresh_snapshot(header);
    }
    header as *mut c_void
}

/// Remove `key` from `set` IN PLACE and return the same header (absent element: no-op).
#[unsafe(no_mangle)]
pub extern "C" fn __set_remove(
    set: *const c_void,
    tag: i64,
    a: i64,
    b: i64,
    hash_fn: *const c_void,
    eq_fn: *const c_void,
) -> *mut c_void {
    let header = set as *mut QnSet;
    unsafe {
        (*header)
            .table
            .remove(&QnKey::new(tag, a, b, hash_fn, eq_fn));
        refresh_snapshot(header);
    }
    header as *mut c_void
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
    let set = set as *const QnSet;
    unsafe {
        (*set)
            .table
            .contains(&QnKey::new(tag, a, b, hash_fn, eq_fn)) as i64
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn __set_len(set: *const c_void) -> i64 {
    unsafe { (*(set as *const QnSet)).len }
}

#[unsafe(no_mangle)]
pub extern "C" fn __set_item_a(set: *const c_void, i: i64) -> i64 {
    unsafe { (*(*(set as *const QnSet)).snapshot.add(i as usize)).key_a as i64 }
}

#[unsafe(no_mangle)]
pub extern "C" fn __set_item_b(set: *const c_void, i: i64) -> i64 {
    unsafe { (*(*(set as *const QnSet)).snapshot.add(i as usize)).key_b as i64 }
}

#[unsafe(no_mangle)]
pub extern "C" fn __set_union(left: *const c_void, right: *const c_void) -> *mut c_void {
    let (left, right) = (left as *const QnSet, right as *const QnSet);
    let mut table = unsafe { (*left).table.clone() };
    for key in unsafe { (*right).table.iter() } {
        table.insert(*key);
    }
    unsafe { build_set(table) as *mut c_void }
}

/// A new set of `left`'s elements filtered by membership in `right`: `keep_present` true
/// keeps those present in `right` (intersection), false those absent (difference).
unsafe fn set_filter(left: *const c_void, right: *const c_void, keep_present: bool) -> *mut c_void {
    let (left, right) = (left as *const QnSet, right as *const QnSet);
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
