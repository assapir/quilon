~ core.test — assertions for self-verifying programs. Import with `<< core.test`.
~ Reference: docs/corelib/test.md.
~
~ A failing assertion reports at YOUR call site, then exits 101.
~
~   assert(cond)          on a false `cond`, report a default message and exit 101;
~                         on true, do nothing. Returns `$` (Unit).
~   assert(cond, opts)    same, but report `opts.message`.
~   AssertOpts            options record for `assert`: { message :: Text }. Records are
~                         nominal — construct by name: `AssertOpts { message = "…" }`.
~   assertEq(a, b)        assert `a == b` (over Num / Text / Bool); reports both values.
~   assertNotEq(a, b)     assert `a != b`; reports the (equal) value.
~   assertOk(r)           assert a Result is `Ok`.
~   assertNotOk(r)        assert a Result is `NotOk`.
~   failAt(message)       fail outright at the CALLER's location — what the assertions
~                         above are built from. An assertion of your own reports ITS
~                         caller by taking a trailing `site :: Site` and forwarding it:
~                           assertEven = (n :: Num, site :: Site) -> $ =>
~                             n % 2 == 0 ? $ : failAt("`n` is odd", site)
<< core.io

~ Options record for `assert`; `message` is the text reported on failure.
>> AssertOpts = { message :: Text }

~ Report `message` at `site` — the location of the call that left the `site` argument off —
~ in the compiler's diagnostic frame, then exit 101 (distinct from the small result codes a
~ program returns from `^`).
>> failAt = (message :: Text, site :: Site) -> $ => <
  color = __color_enabled(stderr)
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

  ~ A path longer than this is shown from its END behind a `…`. Keep the width in step with
  ~ `shorten_path` in the runtime, which shortens a fail-loud report's path the same way.
  room = 60
  file = site.file.length > room
    ? "…" + site.file.slice(site.file.length - room + 1, site.file.length)
    : site.file

  ~ Position first, then the message on its own line.
  eprint("`position``file`:`site.line`:`site.column`:`plain`")
  eprint("`problem``message``plain`")
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
