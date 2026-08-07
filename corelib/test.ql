~ core.test — assertions for self-verifying programs and examples.
~ Import with `<< core.test`. Exposes:
~   assert(cond)          ~ the PRIMITIVE: on a false condition, print a failure
~                           message to stderr and exit with code 101 (so CI fails);
~                           on true, do nothing. Returns `$` (Unit).
~   assertEq(actual, expected)  ~ assert actual == expected; on failure prints both.
~   assertNotEq(a, b)     ~ assert a != b; on failure prints the (equal) value.
~   assertOk(r)           ~ assert a Result is Ok.
~   assertNotOk(r)        ~ assert a Result is NotOk.
~
~ Example:
~   << core.test
~   ^ = () -> $ => <
~     assert(1 + 1 == 2)
~     assertEq(6 * 7, 42)
~     assertNotEq("a", "b")
~     assertOk([1, 2].at(0))
~   >
~
~ Design:
~   The ENTIRE module is pure Quilon except a single native primitive `__exit(code)`
~   (the process-exit intrinsic — Quilon cannot yet exit/abort mid-program
~   in-language). `assert` and every wrapper are ordinary `.ql` built on `assert`,
~   `==`/`!=`/pattern-match, and `eprint` (from core.io). Port `__exit` away — or wrap
~   it in a language-level exit — later; for now it is `__`-prefixed to mark it
~   internal (there is no user-facing `exit`).
~
~   Value rendering uses `eprint`, so it works for `Num`/`Text`/`Bool`; other types
~   (records, arrays, sum payloads) get only the generic "assertion failed" line until
~   a `toText` exists. `assertEq`/`assertNotEq` are overload sets — one member per
~   comparable scalar type — because Quilon has no generics (the only polymorphism is
~   exact-type overloading, and `==` is defined over Num / Text / Bool).
~
~   Each definition is a `< >` block that prints context on the failing path (via a
~   ternary/match whose arms are single `eprint`/`$`/`__exit` expressions — a `< >`
~   block can't itself be a ternary/match arm) and otherwise falls through. On success
~   nothing prints; on failure the context is printed and the process exits non-zero.
~
~   Exit code 101 is the Rust-panic convention, chosen to stand apart from the small
~   result codes examples use as their normal exit status.
<< core.io

~ The primitive. `cond :: Bool`; a false condition prints "assertion failed" to stderr
~ and exits 101 (so CI fails). On true it does nothing. Everything else is built on it.
>> assert = (cond :: Bool) -> $ => <
  cond ? $ : eprint("assertion failed")
  cond ? $ : __exit(101)
  $
>

~ assertEq(actual, expected) — assert the two are equal; on failure print expected
~ then actual to stderr, then abort. One member per scalar type (`==` is defined over
~ Num / Text / Bool).
>> assertEq = (actual :: Num, expected :: Num) -> $ => <
  eq = actual == expected
  eq ? $ : eprint("assertEq failed — expected:")
  eq ? $ : eprint(expected)
  eq ? $ : eprint("             got actual:")
  eq ? $ : eprint(actual)
  assert(eq)
>

>> assertEq = (actual :: Text, expected :: Text) -> $ => <
  eq = actual == expected
  eq ? $ : eprint("assertEq failed — expected:")
  eq ? $ : eprint(expected)
  eq ? $ : eprint("             got actual:")
  eq ? $ : eprint(actual)
  assert(eq)
>

>> assertEq = (actual :: Bool, expected :: Bool) -> $ => <
  eq = actual == expected
  eq ? $ : eprint("assertEq failed — expected:")
  eq ? $ : eprint(expected)
  eq ? $ : eprint("             got actual:")
  eq ? $ : eprint(actual)
  assert(eq)
>

~ assertNotEq(a, b) — assert the two differ; on failure print the (equal) value.
>> assertNotEq = (a :: Num, b :: Num) -> $ => <
  ne = a != b
  ne ? $ : eprint("assertNotEq failed — both values equal:")
  ne ? $ : eprint(a)
  assert(ne)
>

>> assertNotEq = (a :: Text, b :: Text) -> $ => <
  ne = a != b
  ne ? $ : eprint("assertNotEq failed — both values equal:")
  ne ? $ : eprint(a)
  assert(ne)
>

>> assertNotEq = (a :: Bool, b :: Bool) -> $ => <
  ne = a != b
  ne ? $ : eprint("assertNotEq failed — both values equal:")
  ne ? $ : eprint(a)
  assert(ne)
>

~ assertOk(r) — assert a Result is Ok; abort on NotOk.
>> assertOk = (r :: Result) -> $ => <
  ok = r ? | Ok(_) => true | NotOk(_) => false
  ok ? $ : eprint("assertOk failed — expected Ok, got NotOk")
  assert(ok)
>

~ assertNotOk(r) — assert a Result is NotOk; abort on Ok.
>> assertNotOk = (r :: Result) -> $ => <
  ok = r ? | Ok(_) => true | NotOk(_) => false
  ok ? eprint("assertNotOk failed — expected NotOk, got Ok") : $
  assert(!ok)
>
