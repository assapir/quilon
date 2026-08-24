~ User-defined sum types use `/` as the variant separator. Variants may be
~ nullary (`Red`) or carry built-in-typed payloads (`Rect(Num, Num)`, `Ok($)`).
~ Pattern-match with `?`/`|` to dispatch on the variant and bind its payload; the
~ match must be exhaustive. `Result` (`Ok`/`NotOk`) is just a predefined sum type.
~ `<< core.test` verifies every result; on success the program exits 0.
<< core.test

~ A nullary enum.
Color = Red / Green / Blue

~ Map each color to a number.
rank = (c :: Color) -> Num => c ?
  | Red   => 0
  | Green => 1
  | Blue  => 2

~ A sum type with payload variants (consistent payload types per position).
Shape = Circle(Num) / Rect(Num, Num)

area = (s :: Shape) -> Num => s ?
  | Circle(r)  => 3 * r * r          ~ ~pi*r^2 (pi ~= 3 here, integer-only)
  | Rect(w, h) => w * h

~ `Ok($)` is the canonical "succeeded, no meaningful value" Result; `NotOk(code)`
~ carries a failure code. Matching `Ok(_)` ignores the unit payload.
check = (n :: Num) -> Result => n <= 100 ? Ok($) : NotOk(n)

status = (n :: Num) -> Num => check(n) ?
  | Ok(_)    => 0
  | NotOk(c) => c

^ = () -> $ => <
  assertEq(area(Rect(6, 7)), 42)   ~ 6 * 7
  assertEq(rank(Green), 1)
  assertEq(status(50), 0)          ~ 50 <= 100 -> Ok($) -> 0
  assertEq(status(200), 200)       ~ 200 > 100 -> NotOk(200) -> 200
>
