//! Built-in `Map` and `Set` collection types: the pipe-fenced literals `[|K => V|]` /
//! `[|T|]`, keyed access (`m[k]` fail-loud, `.get` safe), the lean method surface, and
//! the set-algebra operators. These drive the full pipeline (lex -> parse -> typecheck ->
//! codegen -> JIT) and assert the real exit code, plus a batch of rejection cases.

mod common;
use common::{assert_exit, assert_type_error};
use std::io::Write;
use std::process::Command;

/// Run `source` through the real `quilon` binary as a subprocess, returning
/// `(exit_code, stderr)`. Used for the fail-loud `m[k]` crash, whose `__exit` would
/// otherwise take the in-process JIT harness down with it (mirrors `index_checks_test`).
fn run_subprocess(name: &str, source: &str) -> (i32, String) {
    let mut path = std::env::temp_dir();
    path.push(format!("quilon_col_{}_{}.ql", std::process::id(), name));
    let mut f = std::fs::File::create(&path).expect("create temp .ql");
    f.write_all(source.as_bytes()).expect("write temp .ql");

    let out = Command::new(env!("CARGO_BIN_EXE_quilon"))
        .arg("run")
        .arg(&path)
        .output()
        .expect("run quilon");

    let _ = std::fs::remove_file(&path);
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

// ---- Map: literals, indexing, membership --------------------------------

/// A map literal, then fail-loud `m[k]` lookup of a present key.
#[test]
fn map_literal_and_index() {
    assert_exit(
        "^ = () -> Num => <\n  m :: [|Text => Num|] = [|\"a\" => 10, \"b\" => 20|]\n  m[\"a\"] + m[\"b\"]\n>",
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
        "^ = () -> Num => <\n  m :: [|Text => Num|] = [|\"a\" => 1, \"a\" => 9|]\n  m.size * 100 + m[\"a\"]\n>",
        109,
    );
}

// ---- Map: get returns a Result ------------------------------------------

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

// ---- Map: immutability, set, keys, values -------------------------------

/// `.set` returns a NEW map; the receiver is unchanged (persistent/immutable).
#[test]
fn map_set_is_persistent() {
    assert_exit(
        "^ = () -> Num => <\n  m :: [|Text => Num|] = [|\"a\" => 1|]\n  m2 :: [|Text => Num|] = m.set(\"a\", 5)\n  m[\"a\"] * 10 + m2[\"a\"]\n>",
        15,
    );
}

/// `.set` of a new key grows the map (a fresh map of size+1).
#[test]
fn map_set_new_key_grows() {
    assert_exit(
        "^ = () -> Num => <\n  m :: [|Text => Num|] = [|\"a\" => 1|]\n  m.set(\"b\", 2).size\n>",
        2,
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

// ---- Map: each, key kinds ------------------------------------------------

/// `.each((k, v) => ...)` runs for effect and returns the receiver (so it chains).
#[test]
fn map_each_effect_and_chains() {
    assert_exit(
        "^ = () -> Num => <\n  m :: [|Text => Num|] = [|\"a\" => 10, \"b\" => 20|]\n  sum := 0\n  m.each((k, v) => <\n    sum := sum + v\n  >\n  )\n  sum + m.each((k, v) => v).size\n>",
        // sum 30 ; chained .each returns the map, .size = 2 -> 32
        32,
    );
}

/// Num keys work (hashed by value).
#[test]
fn map_num_keys() {
    assert_exit(
        "^ = () -> Num => <\n  m :: [|Num => Num|] = [|1 => 100, 2 => 200|]\n  m[1] + m[2]\n>",
        300,
    );
}

/// Bool keys work.
#[test]
fn map_bool_keys() {
    assert_exit(
        "^ = () -> Num => <\n  m :: [|Bool => Num|] = [|true => 3, false => 4|]\n  m[true] * 10 + m[false]\n>",
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

/// `m[k]` on a MISSING key is fail-loud: a clear stderr message and exit status 1, never
/// the `99` the program would otherwise return. Driven as a subprocess (the crash's
/// `__exit` would take an in-process JIT run down with it).
#[test]
fn map_index_missing_key_crashes() {
    let (code, stderr) = run_subprocess(
        "missing",
        "^ = () -> Num => <\n  m :: [|Text => Num|] = [|\"a\" => 1|]\n  x :: Num = m[\"missing\"]\n  99\n>",
    );
    assert_eq!(code, 1, "missing key must exit 1, got {code}: {stderr}");
    assert!(
        stderr.contains("map key \"missing\" not found"),
        "stderr must name the missing key, got: {stderr}"
    );
}

// ---- Set: literals, membership, add -------------------------------------

/// A set literal, `.has`, and `.size`; a duplicate element is collapsed.
#[test]
fn set_literal_has_and_size() {
    assert_exit(
        "^ = () -> Num => <\n  s :: [|Num|] = [|1, 2, 2, 3|]\n  s.size * 10 + (s.has(2) ? 1 : 0)\n>",
        // unique {1,2,3} -> size 3 -> 30 ; has(2) -> 1 -> 31
        31,
    );
}

/// `.add` returns a NEW set (persistent); the receiver is unchanged.
#[test]
fn set_add_is_persistent() {
    assert_exit(
        "^ = () -> Num => <\n  s :: [|Num|] = [|1, 2|]\n  s2 :: [|Num|] = s.add(3)\n  s.size * 10 + s2.size\n>",
        23,
    );
}

/// `.items()` is a `[]T` array of the elements.
#[test]
fn set_items_is_array() {
    assert_exit(
        "^ = () -> Num => <\n  [|5, 6, 7|].items().size\n>",
        3,
    );
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

// ---- Set: algebra operators ---------------------------------------------

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

// ---- Rejections ----------------------------------------------------------

/// Indexing a map with the wrong key type is a type error.
#[test]
fn map_index_wrong_key_type_rejected() {
    assert_type_error("^ = () -> Num => <\n  m :: [|Text => Num|] = [|\"a\" => 1|]\n  m[1]\n>");
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

/// A set cannot be indexed with `[]` (only arrays and maps can).
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
