//! Built-in `Map` and `Set` collection types: the pipe-fenced literals `[|K => V|]` /
//! `[|T|]`, safe keyed access (`.get`, returning a `Result`), the lean method surface, and
//! the set-algebra operators. These drive the full pipeline (lex -> parse -> typecheck ->
//! codegen -> JIT) and assert the real exit code, plus a batch of rejection cases.

mod common;
use common::{assert_exit, assert_type_error};

/// A map literal, then `.get` lookup of present keys (the only way to read a value).
#[test]
fn map_literal_and_get() {
    assert_exit(
        "^ = () -> Num => <\n  m :: [|Text => Num|] = [|\"a\" => 10, \"b\" => 20|]\n  (m.get(\"a\") ? | Ok(v) => v | NotOk(_) => 0) + (m.get(\"b\") ? | Ok(v) => v | NotOk(_) => 0)\n>",
        30,
    );
}

/// `.has` reports membership; the value type is `Bool`.
#[test]
fn map_has_membership() {
    assert_exit(
        "^ = () -> Num => <\n  m :: [|Text => Num|] = [|\"a\" => 1|]\n  (m.has(\"a\") ? 5 : 0) + (m.has(\"z\") ? 100 : 0)\n>",
        5,
    );
}

/// `.size` is the entry count (a field, like an array's `.size`).
#[test]
fn map_size_is_entry_count() {
    assert_exit(
        "^ = () -> Num => <\n  [|\"a\" => 1, \"b\" => 2, \"c\" => 3|].size\n>",
        3,
    );
}

/// A duplicate key keeps the LAST value (`insert` overwrites), so `size` counts uniques.
#[test]
fn map_duplicate_key_overwrites() {
    assert_exit(
        "^ = () -> Num => <\n  m :: [|Text => Num|] = [|\"a\" => 1, \"a\" => 9|]\n  m.size * 100 + (m.get(\"a\") ? | Ok(v) => v | NotOk(_) => 0)\n>",
        109,
    );
}

/// `.get` yields `Ok(value)` for a present key.
#[test]
fn map_get_ok_present() {
    assert_exit(
        "^ = () -> Num => <\n  [|\"x\" => 7|].get(\"x\") ?\n    | Ok(v)    => v\n    | NotOk(_) => 0\n>",
        7,
    );
}

/// `.get` yields `NotOk` for an absent key.
#[test]
fn map_get_notok_absent() {
    assert_exit(
        "^ = () -> Num => <\n  [|\"x\" => 7|].get(\"z\") ?\n    | Ok(v)    => v\n    | NotOk(_) => 42\n>",
        42,
    );
}

/// `.set` mutates the receiver in place and returns it: the write shows up through the
/// SAME binding.
#[test]
fn map_set_mutates_in_place() {
    assert_exit(
        "^ = () -> Num => <\n  m :: [|Text => Num|] := [|\"a\" => 1|]\n  m.set(\"a\", 5)\n  m.get(\"a\") ? | Ok(v) => v | NotOk(_) => 0\n>",
        5,
    );
}

/// `.set` of a new key grows the map (size+1), and its return chains (it is the mutated
/// receiver).
#[test]
fn map_set_new_key_grows() {
    assert_exit(
        "^ = () -> Num => <\n  m :: [|Text => Num|] := [|\"a\" => 1|]\n  m.set(\"b\", 2).size\n>",
        2,
    );
}

/// `.remove` mutates the receiver in place, dropping the key.
#[test]
fn map_remove_mutates_in_place() {
    assert_exit(
        "^ = () -> Num => <\n  m :: [|Text => Num|] := [|\"a\" => 1, \"b\" => 2|]\n  m.remove(\"a\")\n  m.size\n>",
        1,
    );
}

/// `.remove` of an absent key is a no-op that still returns the (unchanged-size) receiver.
#[test]
fn map_remove_absent_key_is_noop() {
    assert_exit(
        "^ = () -> Num => <\n  m :: [|Text => Num|] := [|\"a\" => 1, \"b\" => 2|]\n  m.remove(\"z\").size\n>",
        2,
    );
}

/// After `.remove`, the key is gone: `.get` on it yields `NotOk`.
#[test]
fn map_remove_then_get_is_notok() {
    assert_exit(
        "^ = () -> Num => <\n  m :: [|Text => Num|] := [|\"a\" => 1|]\n  m.remove(\"a\").get(\"a\") ?\n    | Ok(v)    => v\n    | NotOk(_) => 42\n>",
        42,
    );
}

/// `.keys()` and `.values()` are arrays; `.values().reduce` folds them.
#[test]
fn map_keys_and_values_are_arrays() {
    assert_exit(
        "^ = () -> Num => <\n  m :: [|Text => Num|] = [|\"a\" => 1, \"b\" => 2, \"c\" => 3|]\n  m.keys().size * 100 + m.values().reduce(0, (acc, x) => acc + x)\n>",
        // 3 keys -> 300 ; values 1+2+3 = 6 ; total 306
        306,
    );
}

/// `.each((k, v) => ...)` runs for effect and returns the receiver (so it chains).
#[test]
fn map_each_effect_and_chains() {
    assert_exit(
        "^ = () -> Num => <\n  m :: [|Text => Num|] = [|\"a\" => 10, \"b\" => 20|]\n  sum := 0\n  m.each((k, v) => <\n    sum := sum + v\n  >\n  )\n  sum + m.each((k, v) => v).size\n>",
        // sum 30 ; chained .each returns the map, .size = 2 -> 32
        32,
    );
}

/// `each` visits the entries present when it starts: removing every key from inside the
/// callback still visits all of them, and leaves the map empty afterward.
#[test]
fn map_each_removing_every_key_visits_all_entries_first() {
    assert_exit(
        "^ = () -> Num => <\n  m :: [|Text => Num|] := [|\"a\" => 1, \"b\" => 2, \"c\" => 3, \"d\" => 4|]\n  visits := 0\n  m.each((k, v) => <\n    visits := visits + 1\n    m.remove(k)\n  >\n  )\n  visits * 100 + m.size\n>",
        400,
    );
}

/// `each`'s walk is fixed at the start: a `set` of a NEW key from inside the callback
/// grows the map but is not itself visited by this same walk.
#[test]
fn map_each_setting_new_keys_visits_only_the_original_entries() {
    assert_exit(
        "^ = () -> Num => <\n  m :: [|Num => Num|] := [|1 => 10, 2 => 20|]\n  visits := 0\n  m.each((k, v) => <\n    visits := visits + 1\n    m.set(k + 100, v)\n  >\n  )\n  visits * 100 + m.size\n>",
        // 2 original entries visited; 2 more added -> size 4.
        204,
    );
}

/// Num keys work (hashed by value).
#[test]
fn map_num_keys() {
    assert_exit(
        "^ = () -> Num => <\n  m :: [|Num => Num|] = [|1 => 100, 2 => 200|]\n  (m.get(1) ? | Ok(v) => v | NotOk(_) => 0) + (m.get(2) ? | Ok(v) => v | NotOk(_) => 0)\n>",
        300,
    );
}

/// Bool keys work.
#[test]
fn map_bool_keys() {
    assert_exit(
        "^ = () -> Num => <\n  m :: [|Bool => Num|] = [|true => 3, false => 4|]\n  (m.get(true) ? | Ok(v) => v | NotOk(_) => 0) * 10 + (m.get(false) ? | Ok(v) => v | NotOk(_) => 0)\n>",
        34,
    );
}

/// An empty map literal `[|=>|]` has size 0.
#[test]
fn map_empty_literal() {
    assert_exit(
        "^ = () -> Num => <\n  m :: [|Num => Num|] = [|=>|]\n  m.size\n>",
        0,
    );
}

/// `-0.0` and `+0.0` are the SAME Num key (the runtime canonicalizes the bits), matching
/// float `==` — a computed `-0.0` must not silently miss a `+0.0` entry.
#[test]
fn map_negative_zero_key_unifies_with_positive_zero() {
    assert_exit(
        "^ = () -> Num => <\n  negz :: Num = 0.0 * (0 - 1)\n  m :: [|Num => Num|] = [|negz => 7|]\n  (m.get(0) ? | Ok(v) => v | NotOk(_) => 0) + (m.has(0) ? 1 : 0)\n>",
        // stored under -0.0, found under +0.0 -> 7 + 1
        8,
    );
}

/// A NaN Num key is canonicalized to one self-equal key, so it is usable and dedupes
/// (rather than being as-many-keys-as-bit-patterns).
#[test]
fn map_nan_key_is_findable_and_dedupes() {
    assert_exit(
        "^ = () -> Num => <\n  nan :: Num = 0.0 / 0.0\n  m :: [|Num => Num|] := [|nan => 9|]\n  m.set(0.0 / 0.0, 3)\n  m.size * 10 + (m.get(0.0 / 0.0) ? | Ok(v) => v | NotOk(_) => 0)\n>",
        // the second NaN key overwrites the first -> size 1 -> 10 + 3
        13,
    );
}

/// A set literal, `.has`, and `.size`; a duplicate element is collapsed.
#[test]
fn set_literal_has_and_size() {
    assert_exit(
        "^ = () -> Num => <\n  s :: [|Num|] = [|1, 2, 2, 3|]\n  s.size * 10 + (s.has(2) ? 1 : 0)\n>",
        // unique {1,2,3} -> size 3 -> 30 ; has(2) -> 1 -> 31
        31,
    );
}

/// `.add` mutates the receiver in place and returns it: the write shows up through the
/// SAME binding.
#[test]
fn set_add_mutates_in_place() {
    assert_exit(
        "^ = () -> Num => <\n  s :: [|Num|] := [|1, 2|]\n  s.add(3)\n  s.size\n>",
        3,
    );
}

/// `.remove` mutates the receiver in place, dropping the element.
#[test]
fn set_remove_mutates_in_place() {
    assert_exit(
        "^ = () -> Num => <\n  s :: [|Num|] := [|1, 2, 3|]\n  s.remove(2)\n  s.size\n>",
        2,
    );
}

/// `.remove` of an absent element is a no-op that still returns the (unchanged-size)
/// receiver.
#[test]
fn set_remove_absent_element_is_noop() {
    assert_exit(
        "^ = () -> Num => <\n  s :: [|Num|] := [|1, 2, 3|]\n  s.remove(9).size\n>",
        3,
    );
}

/// After `.remove`, membership reports the element gone.
#[test]
fn set_remove_then_has_is_false() {
    assert_exit(
        "^ = () -> Num => <\n  s :: [|Num|] := [|1, 2|]\n  s.remove(1).has(1) ? 5 : 0\n>",
        0,
    );
}

/// `.items()` is a `[]T` array of the elements.
#[test]
fn set_items_is_array() {
    assert_exit("^ = () -> Num => <\n  [|5, 6, 7|].items().size\n>", 3);
}

/// An empty set literal `[||]` has size 0.
#[test]
fn set_empty_literal() {
    assert_exit("^ = () -> Num => <\n  s :: [|Num|] = [||]\n  s.size\n>", 0);
}

/// `.each` runs for effect and returns the receiver.
#[test]
fn set_each_effect_and_chains() {
    assert_exit(
        "^ = () -> Num => <\n  s :: [|Num|] = [|4, 5, 6|]\n  sum := 0\n  s.each(x => <\n    sum := sum + x\n  >\n  )\n  sum + s.each(x => x).size\n>",
        // sum 15 ; chained .size 3 -> 18
        18,
    );
}

/// `each` visits the elements present when it starts: removing every element from inside
/// the callback still visits all of them, and leaves the set empty afterward.
#[test]
fn set_each_removing_every_element_visits_all_entries_first() {
    assert_exit(
        "^ = () -> Num => <\n  s :: [|Num|] := [|1, 2, 3, 4|]\n  visits := 0\n  s.each(x => <\n    visits := visits + 1\n    s.remove(x)\n  >\n  )\n  visits * 100 + s.size\n>",
        400,
    );
}

/// `+` is union.
#[test]
fn set_union() {
    assert_exit(
        "^ = () -> Num => <\n  ([|1, 2, 3|] + [|3, 4, 5|]).size\n>",
        // {1,2,3,4,5}
        5,
    );
}

/// `-` is difference.
#[test]
fn set_difference() {
    assert_exit(
        "^ = () -> Num => <\n  ([|1, 2, 3|] - [|3, 4, 5|]).size\n>",
        // {1,2}
        2,
    );
}

/// `+-` and `-+` are the SAME symmetric intersection operator.
#[test]
fn set_intersection_both_spellings() {
    assert_exit(
        "^ = () -> Num => <\n  a :: [|Num|] = [|1, 2, 3|]\n  b :: [|Num|] = [|3, 4, 5|]\n  (a +- b).size * 10 + (a -+ b).size\n>",
        // both {3} -> size 1 each -> 11
        11,
    );
}

/// A map has no bracket indexing at all: values are read only through `.get` (which
/// returns a `Result`). Even a correctly-typed key is rejected by the checker.
#[test]
fn map_index_rejected() {
    assert_type_error("^ = () -> Num => <\n  m :: [|Text => Num|] = [|\"a\" => 1|]\n  m[\"a\"]\n>");
}

/// A map literal whose values disagree in type is rejected.
#[test]
fn map_mixed_value_types_rejected() {
    assert_type_error("^ = () -> Num => <\n  [|\"a\" => 1, \"b\" => \"two\"|].size\n>");
}

/// A map literal whose keys disagree in type is rejected.
#[test]
fn map_mixed_key_types_rejected() {
    assert_type_error("^ = () -> Num => <\n  [|\"a\" => 1, 2 => 2|].size\n>");
}

/// A non-hashable key type (here an array) is rejected.
#[test]
fn set_non_hashable_element_rejected() {
    assert_type_error("^ = () -> Num => <\n  [|[1, 2], [3, 4]|].size\n>");
}

/// A set cannot be indexed with `[]` (only arrays can).
#[test]
fn set_index_rejected() {
    assert_type_error("^ = () -> Num => <\n  s :: [|Num|] = [|1, 2|]\n  s[0]\n>");
}

/// The intersection operator on non-set operands is a type error.
#[test]
fn intersection_on_nums_rejected() {
    assert_type_error("^ = () -> Num => <\n  1 +- 2\n>");
}

/// `+` on mismatched set element types is rejected.
#[test]
fn set_union_mismatched_elem_types_rejected() {
    assert_type_error("^ = () -> Num => <\n  ([|1, 2|] + [|\"a\", \"b\"|]).size\n>");
}

// --- User-defined key types: a type with both a `%` hash hook and an `==` member ---

/// A Map keyed by a user record type: `.has`/`.get`/`.set` resolve by the key's `%`/`==`.
#[test]
fn map_user_record_key() {
    assert_exit(
        r#"Point = {
  x :: Num, y :: Num,
  == = (other :: Point) -> Bool => < it.x == other.x && it.y == other.y >,
  % = () -> Num => < it.x * 31 + it.y >
}
^ = () -> Num => <
  m :: [|Point => Num|] = [|Point { x = 1, y = 2 } => 10, Point { x = 3, y = 4 } => 20|]
  hit :: Num = m.get(Point { x = 3, y = 4 }) ? | Ok(v) => v | NotOk(_) => 0
  m.size * 1000 + hit * 10 + (m.has(Point { x = 9, y = 9 }) ? 1 : 0)
>"#,
        2200,
    );
}

/// A user-record key survives an in-place `.set` (new key grows) and `.remove`.
#[test]
fn map_user_record_key_set_and_remove_mutate_in_place() {
    assert_exit(
        r#"Point = {
  x :: Num, y :: Num,
  == = (other :: Point) -> Bool => < it.x == other.x && it.y == other.y >,
  % = () -> Num => < it.x * 31 + it.y >
}
^ = () -> Num => <
  m :: [|Point => Num|] := [|Point { x = 1, y = 2 } => 10|]
  sizeAfterSet :: Num = m.set(Point { x = 5, y = 6 }, 30).size
  m.remove(Point { x = 1, y = 2 })
  sizeAfterRemove :: Num = m.size
  hasOld :: Bool = m.has(Point { x = 1, y = 2 })
  sizeAfterSet * 100 + sizeAfterRemove * 10 + (hasOld ? 1 : 0)
>"#,
        210,
    );
}

/// A Set of a user record type deduplicates, and `.remove` mutates it in place by `%`/`==`.
#[test]
fn set_user_record_key_dedup_and_remove_mutates_in_place() {
    assert_exit(
        r#"Point = {
  x :: Num, y :: Num,
  == = (other :: Point) -> Bool => < it.x == other.x && it.y == other.y >,
  % = () -> Num => < it.x * 31 + it.y >
}
^ = () -> Num => <
  s :: [|Point|] := [|Point { x = 1, y = 1 }, Point { x = 2, y = 2 }, Point { x = 1, y = 1 }|]
  sizeBeforeRemove :: Num = s.size
  s.remove(Point { x = 1, y = 1 })
  sizeAfterRemove :: Num = s.size
  hasOther :: Bool = s.has(Point { x = 2, y = 2 })
  sizeBeforeRemove * 100 + sizeAfterRemove * 10 + (hasOther ? 1 : 0)
>"#,
        211,
    );
}

/// A Map keyed by a user SUM type crosses the ABI by-value struct (not by-pointer).
#[test]
fn map_user_sum_key() {
    assert_exit(
        r#"Shape = Circle(Num) / Rect(Num, Num) {
  area = () -> Num => < it ? | Circle(r) => 3 * r * r | Rect(w, h) => w * h >
  == = (other :: Shape) -> Bool => < it.area() == other.area() >
  % = () -> Num => < it.area() >
}
^ = () -> Num => <
  m :: [|Shape => Num|] = [|Circle(4) => 10, Rect(6, 7) => 20|]
  v :: Num = m.get(Rect(6, 7)) ? | Ok(n) => n | NotOk(_) => 0
  m.size * 100 + v
>"#,
        220,
    );
}

/// A key type defining `%` but no `==` is rejected (both are required).
#[test]
fn map_key_type_missing_equality_rejected() {
    assert_type_error(
        r#"Point = {
  x :: Num, y :: Num,
  % = () -> Num => < it.x * 31 + it.y >
}
^ = () -> Num => <
  m :: [|Point => Num|] = [|Point { x = 1, y = 2 } => 1|]
  m.size
>"#,
    );
}

/// A key type defining `==` but no `%` hash hook is rejected (both are required).
#[test]
fn set_key_type_missing_hash_rejected() {
    assert_type_error(
        r#"Point = {
  x :: Num, y :: Num,
  == = (other :: Point) -> Bool => < it.x == other.x && it.y == other.y >
}
^ = () -> Num => <
  s :: [|Point|] = [|Point { x = 1, y = 2 }|]
  s.size
>"#,
    );
}

/// The `%` hash hook must return Num.
#[test]
fn hash_hook_non_num_return_rejected() {
    assert_type_error(
        r#"Point = {
  x :: Num,
  == = (other :: Point) -> Bool => < it.x == other.x >,
  % = () -> Text => < "nope" >
}
^ = () -> Num => <
  [|Point { x = 1 }|].size
>"#,
    );
}

/// A binary `%` (modulo) is NOT the unary hash hook: a type defining it (plus `==`) as a
/// key is rejected at the type checker — a clean pairing error, not a codegen symbol miss.
#[test]
fn binary_mod_is_not_a_hash_hook_rejected() {
    assert_type_error(
        r#"Point = {
  x :: Num, y :: Num,
  == = (other :: Point) -> Bool => < it.x == other.x && it.y == other.y >,
  % = (other :: Point) -> Point => < Point { x = it.x, y = it.y } >
}
^ = () -> Num => <
  [|Point { x = 1, y = 2 } => 1|].size
>"#,
    );
}
