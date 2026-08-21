~ core.test — assertions for self-verifying programs. Import with `<< core.test`.
~
~ A failing assertion says WHERE it failed: the file, line, and column of the call in
~ YOUR code, the source line, and a caret under the call —
~
~   demo.ql:12:3: assertion failed: expected 42, got 41
~     |
~  12 |   assertEq(answer(), 42)
~     |   ^^^^^^^^^^^^^^^^^^^^^^
~
~ and exits 101. The location is always the call site in your code, never an internal
~ hop inside this module: every assertion takes a trailing `site :: Site` parameter,
~ which the compiler fills in with the location of the call that left it off, and which
~ each wrapper forwards to `failAt`. The report is colored when stderr is a terminal
~ (`NO_COLOR=1`, `TERM=dumb`, or a redirect turns it off — see core.io's `colorEnabled`).
~
~   assert(cond)          on a false `cond`, report a default message and exit 101;
~                         on true, do nothing. Returns `$` (Unit).
~   assert(cond, opts)    same, but report `opts.message` instead of the default.
~   AssertOpts            options record for `assert`: { message :: Text }. Records
~                         are nominal — construct by name: `AssertOpts { message = "…" }`.
~   assertEq(a, b)        assert `a == b` (over Num / Text / Bool); reports both values.
~   assertNotEq(a, b)     assert `a != b`; reports the (equal) value.
~   assertOk(r)           assert a Result is `Ok`.
~   assertNotOk(r)        assert a Result is `NotOk`.
~   failAt(message)       fail outright at the CALLER's location — the primitive the
~                         assertions above are built from. An assertion of your own that
~                         should report ITS caller takes a trailing `site :: Site` and
~                         forwards it:
~                           assertEven = (n :: Num, site :: Site) -> $ =>
~                             n % 2 == 0 ? $ : failAt("`n` is odd", site)
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

~ Options record for `assert`; `message` is the text reported on failure.
>> AssertOpts = { message :: Text }

~ Report `message` at `site` — the location of the call that left the `site` argument
~ off — and exit 101 (the Rust-panic convention, deliberately distinct from the small
~ result codes a program returns from `^`). The report mirrors a compiler diagnostic:
~ the position, the message, then the source line with a caret run under the call.
>> failAt = (message :: Text, site :: Site) -> $ => <
  color = colorEnabled(stderr)
  ~ ANSI styling, or nothing at all when the reader is not a terminal.
  position = color ? "\e[36m" : ""
  problem = color ? "\e[1;31m" : ""
  frame = color ? "\e[2m" : ""
  plain = color ? "\e[0m" : ""

  ~ The line number sets the gutter width, so the `|` rules line up under it.
  number = "`site.line`"
  gutter = " ".repeat(number.length)
  lead = " ".repeat(site.column - 1)
  carets = "^".repeat(site.width)

  eprint("`position``site.file`:`site.line`:`site.column`:`plain` `problem``message``plain`")
  eprint("`frame``gutter` |`plain`")
  eprint("`frame``number` |`plain` `site.excerpt`")
  eprint("`frame``gutter` |`plain` `lead``problem``carets``plain`")
  __exit(101)
>

~ The primitive: on a false `cond`, report `opts.message` at the caller's location.
>> assert = (cond :: Bool, opts :: AssertOpts, site :: Site) -> $ =>
  cond ? $ : failAt(opts.message, site)

~ `assert(cond)` is `assert` with the default message.
>> assert = (cond :: Bool, site :: Site) -> $ => cond ? $ : failAt("assertion failed", site)

~ Assert `actual == expected`, reporting both values on failure. One member per scalar
~ type (`==` is defined over Num / Text / Bool); Text values are quoted, so a trailing
~ space or an empty string is visible in the report.
>> assertEq = (actual :: Num, expected :: Num, site :: Site) -> $ =>
  actual == expected
    ? $
    : failAt("assertion failed: expected `expected`, got `actual`", site)

>> assertEq = (actual :: Text, expected :: Text, site :: Site) -> $ =>
  actual == expected
    ? $
    : failAt("assertion failed: expected \"`expected`\", got \"`actual`\"", site)

>> assertEq = (actual :: Bool, expected :: Bool, site :: Site) -> $ =>
  actual == expected
    ? $
    : failAt("assertion failed: expected `expected`, got `actual`", site)

~ Assert `a != b`, reporting the (equal) value on failure.
>> assertNotEq = (a :: Num, b :: Num, site :: Site) -> $ =>
  a != b ? $ : failAt("assertion failed: expected a different value, got `a`", site)

>> assertNotEq = (a :: Text, b :: Text, site :: Site) -> $ =>
  a != b ? $ : failAt("assertion failed: expected a different value, got \"`a`\"", site)

>> assertNotEq = (a :: Bool, b :: Bool, site :: Site) -> $ =>
  a != b ? $ : failAt("assertion failed: expected a different value, got `a`", site)

~ Assert a Result is `Ok`; fail on `NotOk`.
>> assertOk = (r :: Result, site :: Site) -> $ =>
  r ? | Ok(_) => $ | NotOk(_) => failAt("assertion failed: expected Ok, got NotOk", site)

~ Assert a Result is `NotOk`; fail on `Ok`.
>> assertNotOk = (r :: Result, site :: Site) -> $ =>
  r ? | Ok(_) => failAt("assertion failed: expected NotOk, got Ok", site) | NotOk(_) => $
