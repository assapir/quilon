// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! Shared backing for the `Map` and `Set` collection types: the ABI key triple, the
//! fixed-seed hasher, and the GC allocator both headers build their snapshots with.

use crate::mem::__alloc;
use std::hash::{BuildHasher, Hash, Hasher};
use std::os::raw::c_void;

// Key-kind tags shared with the compiler ABI (see `KEY_TAG_*` in the code generator's
// `collections.rs`).
pub(crate) const TAG_NUM: u8 = 0;
pub(crate) const TAG_TEXT: u8 = 1;
pub(crate) const TAG_USER: u8 = 3;

/// A monomorphized `%` hash hook for a user key type: loads the boxed key and returns its
/// `Num` hash. The compiler emits one such trampoline per user key type and passes its
/// address across the ABI (the `__run_fiber_main` function-pointer precedent).
pub(crate) type KeyHashFn = extern "C" fn(*const c_void) -> f64;

/// A monomorphized `==` for a user key type: loads both boxed keys and returns `0`/`1`.
/// The `i64` return normalizes the member's `Bool` (an LLVM `i1`) across the C ABI.
pub(crate) type KeyEqFn = extern "C" fn(*const c_void, *const c_void) -> i64;

/// Canonicalize a `Num`'s bits so key equality matches the language's `==` on Num (float
/// `OEQ`): unify `-0.0` with `+0.0` (distinct bit patterns, but `==`), and collapse every
/// NaN to one bit pattern so a NaN key/hash is a single, self-equal value rather than as
/// many keys as bit patterns. Shared by `Num` keys and the hash a user `%` returns.
pub(crate) fn canonical_num_bits(bits: u64) -> u64 {
    let f = f64::from_bits(bits);
    if f == 0.0 {
        0
    } else if f.is_nan() {
        f64::NAN.to_bits()
    } else {
        bits
    }
}

/// A `BuildHasher` with a fixed seed: `DefaultHasher::new()` uses constant SipHash keys
/// (unlike `RandomState`, which randomizes per process), so a map/set iterates the same
/// way on every run — reproducible, though the order is still unspecified by contract.
#[derive(Clone, Default)]
pub(crate) struct FixedState;

impl BuildHasher for FixedState {
    type Hasher = std::collections::hash_map::DefaultHasher;
    fn build_hasher(&self) -> Self::Hasher {
        std::collections::hash_map::DefaultHasher::new()
    }
}

/// A hashable key from the compiler ABI. `Text` keys carry a `(pointer, length)` into GC
/// memory and hash/compare BY CONTENT, consistent with Quilon's value `==` on `Text`;
/// `Num`/`Bool` keys hash by their word bits. A `TAG_USER` key carries the boxed key value
/// in `a` and hashes/compares by calling back into the type's monomorphized `%`/`==`.
#[derive(Clone, Copy)]
pub(crate) struct QlKey {
    pub(crate) tag: u8,
    pub(crate) a: u64,
    pub(crate) b: u64,
    pub(crate) hash_fn: Option<KeyHashFn>,
    pub(crate) eq_fn: Option<KeyEqFn>,
}

impl QlKey {
    pub(crate) fn new(
        tag: i64,
        a: i64,
        b: i64,
        hash_fn: *const c_void,
        eq_fn: *const c_void,
    ) -> QlKey {
        let tag = tag as u8;
        let a = if tag == TAG_NUM {
            canonical_num_bits(a as u64)
        } else {
            a as u64
        };
        let hash_fn = if hash_fn.is_null() {
            None
        } else {
            Some(unsafe { std::mem::transmute::<*const c_void, KeyHashFn>(hash_fn) })
        };
        let eq_fn = if eq_fn.is_null() {
            None
        } else {
            Some(unsafe { std::mem::transmute::<*const c_void, KeyEqFn>(eq_fn) })
        };
        QlKey {
            tag,
            a,
            b: b as u64,
            hash_fn,
            eq_fn,
        }
    }

    /// The bytes a `Text` key points at (empty for a null/zero-length key).
    unsafe fn text_bytes(&self) -> &'static [u8] {
        if self.a == 0 || self.b == 0 {
            return &[];
        }
        unsafe { std::slice::from_raw_parts(self.a as *const u8, self.b as usize) }
    }

    /// The canonicalized bits of the `Num` a user key's `%` hook returns for this key.
    fn user_hash_bits(&self) -> u64 {
        let hash_fn = self.hash_fn.expect("user key without a `%` hash hook");
        canonical_num_bits(hash_fn(self.a as *const c_void).to_bits())
    }
}

impl PartialEq for QlKey {
    fn eq(&self, other: &Self) -> bool {
        if self.tag != other.tag {
            return false;
        }
        match self.tag {
            TAG_TEXT => unsafe { self.text_bytes() == other.text_bytes() },
            TAG_USER => {
                let eq_fn = self.eq_fn.expect("user key without an `==` member");
                eq_fn(self.a as *const c_void, other.a as *const c_void) != 0
            }
            _ => self.a == other.a,
        }
    }
}

impl Eq for QlKey {}

impl Hash for QlKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u8(self.tag);
        match self.tag {
            TAG_TEXT => unsafe { state.write(self.text_bytes()) },
            TAG_USER => state.write_u64(self.user_hash_bits()),
            _ => state.write_u64(self.a),
        }
    }
}

/// In debug builds, verify the user's `%`/`==` agree on `new_key` against every key already
/// present: `x == y` must imply `x.% == y.%`. A mismatch is a bug in the key type (equal
/// keys hashing apart would silently split into duplicate logical entries), so fail loud.
/// The O(n) scan is compiled OUT of release builds.
#[cfg(debug_assertions)]
pub(crate) fn debug_check_user_key<'a>(existing: impl Iterator<Item = &'a QlKey>, new_key: &QlKey) {
    if new_key.tag != TAG_USER {
        return;
    }
    let eq_fn = new_key.eq_fn.expect("user key without an `==` member");
    let new_hash = new_key.user_hash_bits();
    for present in existing {
        if present.tag != TAG_USER {
            continue;
        }
        let equal = eq_fn(new_key.a as *const c_void, present.a as *const c_void) != 0;
        if equal && new_hash != present.user_hash_bits() {
            panic!(
                "Map/Set key consistency violation: two keys are equal by `==` but hash \
                 differently by `%`; a key type's `%` and `==` must agree"
            );
        }
    }
}

/// A no-op in release builds (the consistency scan is debug-only).
#[cfg(not(debug_assertions))]
#[inline]
pub(crate) fn debug_check_user_key<'a>(
    _existing: impl Iterator<Item = &'a QlKey>,
    _new_key: &QlKey,
) {
}

/// GC-allocate space for `count` (at least one) values of type `T`, zeroed.
pub(crate) unsafe fn gc_alloc<T>(count: usize) -> *mut T {
    let bytes = std::mem::size_of::<T>() * count.max(1);
    __alloc(bytes as i64) as *mut T
}
