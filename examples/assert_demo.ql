~ Assertions — `<< core.test`. Every assertion below holds, so the program runs to
~ completion and exits 0. A failing assertion prints a message to stderr and exits 101.
~
~   assert(cond)                ~ fail (exit 101) if cond is false
~   assertEq(actual, expected)  ~ fail unless actual == expected (prints both on failure)
~   assertNotEq(a, b)           ~ fail unless a != b
~   assertOk(r) / assertNotOk(r)~ fail unless the Result is Ok / NotOk
~
~ assertEq/assertNotEq work over Num, Text, and Bool.
<< core.test

^ = () -> $ => <
  ~ The primitive, over a plain Bool.
  assert(1 + 1 == 2)

  ~ Equality across each scalar type.
  assertEq(6 * 7, 42)
  assertEq("qui" + "lon", "quilon")
  assertEq(2 < 3, true)

  ~ Inequality.
  assertNotEq(1, 2)
  assertNotEq("a", "b")

  ~ Results, via the absent-safe `at` (Ok in bounds, NotOk out of bounds).
  assertOk([10, 20, 30].at(0))
  assertNotOk([10, 20, 30].at(9))
>
