# Error messages

Every located failure — a compile error, a failing assertion, a fail-loud runtime check —
prints the same frame: a `path:line:col:` position line, the message on the line under it,
then the offending source line and a caret (`^`) underline beneath the exact span. Line and
column are **1-based** and count characters, not bytes. A path too long for the position line
is shown from its END behind a `…`, so the file name stays visible. For example, the program

```
add = (a :: Num) -> Num => a + true
```

reports (since `+` is an [overload set](../functions/overloading.md), a `Num + Bool` matches no member):

```
program.qn:1:28:
error: No overload of '+' matches argument types (Num, Bool). Candidates: (Num, Num), (Text, Text)
  |
1 | add = (a :: Num) -> Num => a + true
  |                            ^^^^^^^^
```

A multi-line span underlines its first line. A failure with no source location (a missing
file, an unresolved import) prints a plain one-line message. Any compile error exits 1.

Runtime failures use the same frame at the expression responsible: a failing
[assertion](../corelib/test.md) at its own call site, a fail-loud check (a
bad `array[i]`, a violated `Text.replace`/`repeat` contract) at the call that broke the
contract. Reports are colored when stderr is a terminal, and plain when redirected or under
`NO_COLOR`/`TERM=dumb`. Compile errors are not colored yet. A runtime report carries the
source line it names, so that line's text is embedded in the built binary — with no way to
strip it yet.

To stay robust on hostile or machine-generated input, the parser also caps how
deeply expressions may nest: nesting more than **128 levels** of parentheses,
array/record literals, block statements, `[]T` element types, constructor
patterns, or chained prefix operators is a parse error
(`expression nesting too deep …`) rather than a crash.
Ordinary code nests only a handful of levels, so this limit is reached only by
pathological input.
