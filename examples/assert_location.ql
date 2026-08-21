~ What a failing assertion tells you — the one example here that does NOT exit 0.
~
~ It fails on purpose, to show the report: the failing call's own file, line, and column,
~ the source line, and a caret run under the call. The location is where YOU called the
~ assertion, even though `assertEq` fails inside `core.test` several calls deeper — so a
~ failure in a helper points at the helper's line, not at the entry point.
~
~ Run: cargo run -- run examples/assert_location.ql   ~ exits 101, and prints to stderr:
~
~   examples/assert_location.ql:23:3: assertion failed: expected 42, got 41
~      |
~   23 |   assertEq(answer(), 42)
~      |   ^^^^^^^^^^^^^^^^^^^^^^
~
~ Colored when stderr is a terminal; plain when redirected, or under NO_COLOR=1.
~ See `examples/call_site.ql` for the `Site` mechanism this is built on.
<< core.test

~ Off by one, deliberately.
answer = () -> Num => 41

checkTheAnswer = () -> $ => <
  assertEq(answer(), 42)
>

^ = () -> $ => <
  checkTheAnswer()
>
