~ core.test — assertions for self-verifying programs. Import with `<< core.test`.
~
~   assert(cond)          on a false `cond`, print a default message to stderr and
~                         exit non-zero; on true, do nothing. Returns `$` (Unit).
~   assert(cond, opts)    same, but print `opts.message` instead of the default.
~   AssertOpts            options record for `assert`: { message :: Text }. Records
~                         are nominal — construct by name: `AssertOpts { message = "…" }`.
~   assertEq(a, b)        assert `a == b` (over Num / Text / Bool); prints both on failure.
~   assertNotEq(a, b)     assert `a != b`; prints the (equal) value on failure.
~   assertOk(r)           assert a Result is `Ok`.
~   assertNotOk(r)        assert a Result is `NotOk`.
~
~ Example:
~   << core.test
~   ^ = () -> $ => <
~     assert(1 + 1 == 2)
~     assert(1 + 1 == 2, AssertOpts { message = "math is broken" })
~     assertEq(6 * 7, 42)
~     assertNotEq("a", "b")
~     assertOk([1, 2].at(0))
~   >
<< core.io

~ Options record for `assert`; `message` is the text printed on failure.
>> AssertOpts = { message :: Text }

~ The primitive: on a false `cond`, print `opts.message` to stderr and exit 101.
>> assert = (cond :: Bool, opts :: AssertOpts) -> $ => <
  cond ? $ : eprint(opts.message)
  cond ? $ : __exit(101)
  $
>

~ `assert(cond)` is `assert` with the default message.
>> assert = (cond :: Bool) -> $ => assert(cond, AssertOpts { message = "assertion failed" })

~ Assert `actual == expected`; on failure print expected then actual, then abort.
~ One member per scalar type (`==` is defined over Num / Text / Bool).
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

~ Assert `a != b`; on failure print the (equal) value, then abort.
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

~ Assert a Result is `Ok`; abort on `NotOk`.
>> assertOk = (r :: Result) -> $ => <
  ok = r ? | Ok(_) => true | NotOk(_) => false
  ok ? $ : eprint("assertOk failed — expected Ok, got NotOk")
  assert(ok)
>

~ Assert a Result is `NotOk`; abort on `Ok`.
>> assertNotOk = (r :: Result) -> $ => <
  ok = r ? | Ok(_) => true | NotOk(_) => false
  ok ? eprint("assertNotOk failed — expected NotOk, got Ok") : $
  assert(!ok)
>
