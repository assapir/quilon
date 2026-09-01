// Ranges: the infix `<-` operator builds an inclusive `[]Num`.
//   `1 <- 4` -> [1, 2, 3, 4]      (inclusive)
//   `4 <- 1` -> [4, 3, 2, 1]      (descends when the left end is larger)
// It is array sugar — no distinct Range type — so the result composes with
// `.size`, indexing, and the array methods (`.each`). These tests drive the
// full pipeline (lex ->
// parse -> typecheck -> codegen -> JIT) and assert the real exit code.

/// `(1 <- 4).size == 4` — an inclusive range has `|hi - lo| + 1` elements.
/// (`.size` needs a named receiver in 0.9, so bind the range first.)
mod common;
use common::assert_exit;

#[test]
fn range_size_is_inclusive() {
    assert_exit("^ = () -> Num => <\n  r = 1 <- 4\n  r.size\n>", 4);
}

/// A single-point range `5 <- 5` is `[5]` — size 1.
#[test]
fn range_single_point_has_size_one() {
    assert_exit("^ = () -> Num => <\n  r = 5 <- 5\n  r.size\n>", 1);
}

/// Ascending `1 <- 4` is `[1, 2, 3, 4]`: summing the four endpoints by index
/// gives 1 + 2 + 3 + 4 = 10. (Index-summed, not loop-accumulated, so the test
/// is independent of mutable-accumulator behavior.)
#[test]
fn ascending_range_values_in_order() {
    assert_exit(
        "^ = () -> Num => <\n  r = 1 <- 4\n  r[0] + r[1] + r[2] + r[3]\n>",
        10,
    );
}

/// Descending `4 <- 1` is `[4, 3, 2, 1]`: the first element is the LARGER end.
/// Encode the order as 1000*r[0] + 100*r[1] + 10*r[2] + r[3] = 4321.
#[test]
fn descending_range_is_reversed() {
    assert_exit(
        "^ = () -> Num => <\n  r = 4 <- 1\n  1000*r[0] + 100*r[1] + 10*r[2] + r[3]\n>",
        4321,
    );
}

/// Range ends can be dynamic (not just literals): `a <- b` with bound `a`/`b`
/// still materializes correctly, and chooses direction at runtime.
#[test]
fn range_with_dynamic_ends() {
    assert_exit(
        "^ = () -> Num => <\n  a = 2\n  b = 5\n  r = a <- b\n  r.size + r[0] + r[3]\n>",
        // [2,3,4,5]: size 4 + first 2 + last 5 = 11
        11,
    );
}

/// A range is just a `[]Num`, so it iterates with `.each` like any array.
#[test]
fn range_drives_each() {
    // `.each` over [1,2,3] runs for side effects and returns the receiver; then
    // return the size to prove the range materialized.
    assert_exit(
        "^ = () -> Num => <\n  r = 1 <- 3\n  r.each(n => n)\n  r.size\n>",
        3,
    );
}

// ---- Lazy lowering: an array method consuming a range DIRECTLY iterates its bounds ----
// A `lo <- hi` receiver of `.map`/`.filter`/`.reduce` (and a discarded `.each`) is not
// materialized: the method's loop computes each element from the endpoints. Same []Num
// semantics — only the allocation is gone — so these assert results, plus one range far
// too large to materialize (100M elements = 800MB) that must still complete.

/// `.each` over a 100-million-element range completes without allocating it: the sum
/// 1..=100000000 = 5000000050000000 is exact in an f64 (< 2^53).
#[test]
fn each_over_a_huge_range_does_not_materialize() {
    assert_exit(
        "^ = () -> Num => <\n  total := 0\n  (1 <- 100000000).each(n => <\n    total := total + n\n  >)\n  total == 5000000050000000 ? 42 : 1\n>",
        42,
    );
}

/// `.reduce` over the same huge range folds without allocating it.
#[test]
fn reduce_over_a_huge_range_does_not_materialize() {
    assert_exit(
        "^ = () -> Num => <\n  total = (1 <- 100000000).reduce(0, (acc, n) => acc + n)\n  total == 5000000050000000 ? 42 : 1\n>",
        42,
    );
}

/// `.reduce` directly on a small range gives the same fold a bound array would.
#[test]
fn reduce_directly_on_a_range() {
    assert_exit(
        "^ = () -> Num => < (1 <- 4).reduce(0, (acc, n) => acc + n) >",
        10,
    );
}

/// `.map` directly on a range produces the mapped array: ends and size all read back.
#[test]
fn map_directly_on_a_range() {
    // (1 <- 4).map(n => n * 2) = [2, 4, 6, 8]: size 4 + first 2 + last 8 = 14.
    assert_exit(
        "^ = () -> Num => <\n  doubled = (1 <- 4).map(n => n * 2)\n  doubled.size + doubled[0] + doubled[3]\n>",
        14,
    );
}

/// `.filter` directly on a range keeps only the matching elements, in order.
#[test]
fn filter_directly_on_a_range() {
    // (1 <- 10).filter(even) = [2, 4, 6, 8, 10]: size 5 + first 2 + last 10 = 17.
    assert_exit(
        "^ = () -> Num => <\n  evens = (1 <- 10).filter(n => n % 2 == 0)\n  evens.size + evens[0] + evens[4]\n>",
        17,
    );
}

/// A DESCENDING range is lazy too: the loop steps -1 from the left end.
#[test]
fn reduce_on_a_descending_range() {
    // (4 <- 1) folds the same four values: 10; first element seen is 4.
    assert_exit(
        "^ = () -> Num => <\n  first = (4 <- 1).reduce(0, (acc, n) => acc == 0 ? n : acc)\n  sum = (4 <- 1).reduce(0, (acc, n) => acc + n)\n  first * 10 + sum\n>",
        50,
    );
}

/// Every OTHER consumption still materializes: indexing the range expression itself,
/// binding it to a name, and passing it to a function are unchanged.
#[test]
fn other_consumptions_still_materialize() {
    assert_exit(
        "size = (xs :: []Num) -> Num => < xs.size >\n\n^ = () -> Num => <\n  idx = (1 <- 5)[2]\n  bound = 1 <- 5\n  idx + bound.size + size(2 <- 4)\n>",
        // (1<-5)[2] = 3, bound.size = 5, size(2<-4) = 3.
        11,
    );
}

/// A `.each` whose VALUE is used (bound, chained) still yields its receiver array —
/// that position materializes, so Decision 19 chaining is unchanged.
#[test]
fn each_whose_value_is_used_still_yields_the_array() {
    assert_exit(
        "^ = () -> Num => <\n  r = (1 <- 3).each(n => n)\n  r.size + r[2]\n>",
        6,
    );
}
