// SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0

//! Native backing for Quilon's built-in `Map` and `Set` collection types.
//!
//! Each collection is a plain `std::collections::HashMap`/`HashSet` (SwissTable under the
//! hood) wrapped in a GC-allocated header, with a **fixed-seed** `BuildHasher` so
//! iteration is stable run-to-run. Iteration ORDER is unspecified by contract (it is the
//! table's hash order, NOT insertion order); the fixed seed only makes it reproducible so
//! example-asserts don't flake.
//!
//! Mutation: `__map_set`/`__map_remove`/`__set_add`/`__set_remove` mutate the receiver's
//! header IN PLACE (table insert/remove, then a fresh snapshot) and return the same
//! pointer, so the language-level `it` a Quilon `set`/`remove`/`add` call returns is
//! literally the mutated receiver. The set-algebra operators (`+`/`-`/`+-`) stay
//! non-mutating and return a freshly built header.
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
//!
//! The map and set intrinsics live in the `map` and `set` submodules; the ABI key triple,
//! the fixed-seed hasher, and the GC allocator they share live in `common`.

mod common;
mod map;
mod set;

pub(crate) use map::build_text_map;
pub use map::{
    __map_get, __map_has, __map_key_a, __map_key_b, __map_len, __map_new, __map_remove, __map_set,
    __map_val,
};
pub use set::{
    __set_add, __set_diff, __set_has, __set_intersect, __set_item_a, __set_item_b, __set_len,
    __set_new, __set_remove, __set_union,
};
