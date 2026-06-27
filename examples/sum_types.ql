~ User-defined sum types use `/` as the variant separator. Variants may be
~ nullary (`Red`) or carry built-in-typed payloads (`Rect(Num, Num)`, `Ok($)`).
~ Pattern-match with `?`/`|` to dispatch on the variant and bind its payload; the
~ match must be exhaustive. `Result` (`Ok`/`NotOk`) is just a predefined sum type.

~ A nullary enum.
Light = Red / Yellow / Green

~ How long to wait at each light.
wait = (l :: Light) -> Num => l ?
  | Red    => 3
  | Yellow => 1
  | Green  => 0

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

^ = () -> Num => <
  total = area(Rect(6, 7)) + wait(Green)   ~ 42 + 0 = 42
  total + status(50)                         ~ 42 + 0 = 42
>
