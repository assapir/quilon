// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! Shared backing for the `Map` and `Set` collection types: the ABI key triple, the
//! fixed-seed hasher, and the GC allocator both headers build their snapshots with.

use crate::mem::__alloc;
use std::hash::{BuildHasher, Hash, Hasher};

// Key-kind tags shared with the compiler ABI (see `KEY_TAG_*` in the code generator's
// `collections.rs`).
pub(crate) const TAG_NUM: u8 = 0;
pub(crate) const TAG_TEXT: u8 = 1;

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
/// `Num`/`Bool` keys hash by their word bits.
#[derive(Clone, Copy)]
pub(crate) struct QlKey {
    pub(crate) tag: u8,
    pub(crate) a: u64,
    pub(crate) b: u64,
}

impl QlKey {
    pub(crate) fn new(tag: i64, a: i64, b: i64) -> QlKey {
        let tag = tag as u8;
        // Canonicalize a Num key's bits so key equality matches the language's `==` on
        // Num (float `OEQ`): unify `-0.0` with `+0.0` (distinct bit patterns, but `==`),
        // and collapse every NaN to one bit pattern so a NaN key is a single, self-equal
        // key rather than as-many-keys-as-bit-patterns. (Bit-distinct NaN keys, and a
        // `-0.0` that silently misses a `+0.0` entry, are the alternative — both worse.)
        let a = if tag == TAG_NUM {
            let f = f64::from_bits(a as u64);
            if f == 0.0 {
                0
            } else if f.is_nan() {
                f64::NAN.to_bits()
            } else {
                a as u64
            }
        } else {
            a as u64
        };
        QlKey {
            tag,
            a,
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
pub(crate) unsafe fn gc_alloc<T>(count: usize) -> *mut T {
    let bytes = std::mem::size_of::<T>() * count.max(1);
    __alloc(bytes as i64) as *mut T
}
