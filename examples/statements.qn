~ Statement boundaries. Quilon has no statement separator; the grammar is
~ newline-insensitive except for two line-aware rules:
~   1. a line-final `>` closes a block;
~   2. a `(`, `[`, or `{` that is the FIRST token on its line never continues
~      the previous expression — it begins a NEW statement.
~ So call arguments, index brackets, and constructor braces must open on the
~ same line as the expression they apply to, while a continuation line may still
~ start with `.`, `|>`, or an operator. Every assertion below holds, so the
~ program exits 0.
<< core.test

double = (n :: Num) -> Num => n * 2

Point = { x :: Num, y :: Num }

^ = () -> $ => <
  ~ `a` is the array itself: the next line's `[` begins a NEW statement.
  ~ (Without the rule the two lines would fuse into the index `a[3, 4]`.)
  a = [1, 2]
  sum := 0
  [3, 4].each(x => <
    sum := sum + x
  >
  )
  assertEq(a.size, 2)
  assertEq(sum, 7)

  ~ `b` is the call's result: the next line's `(` begins a NEW statement.
  ~ (Without the rule the two lines would fuse into the call `double(4)(1 + 2)`.)
  b = double(4)
  (1 + 2) |> assertEq(3)
  assertEq(b, 8)

  ~ `e` is the record: the next line's `{` begins a NEW statement.
  ~ (Without the rule the two lines would fuse into the constructor `e { x = 9, y = 9 }`.)
  e = Point { x = 5, y = 6 }
  f = e
  { x = 9, y = 9 }
  assertEq(f.x, 5)
  assertEq(f.y, 6)

  ~ Only a LINE-FIRST `(` / `[` / `{` ends the expression: an argument list or a
  ~ constructor body opened on its own line may span lines, and a `.`-led line
  ~ still chains.
  c = double(
    10)
  d = [1, 2, 3]
    .map(x => x * 2)
    .reduce(0, (acc, x) => acc + x)
  g = Point {
    x = 1,
    y = 2
  }
  assertEq(c, 20)
  assertEq(d, 12)
  assertEq(g.x + g.y, 3)
>
