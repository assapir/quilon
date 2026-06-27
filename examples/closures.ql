~ Closures: lexical capture, with the capture mode decided by the binding operator.
~   `=`  bindings are captured BY VALUE     — a frozen, read-only snapshot.
~   `:=` bindings are captured BY REFERENCE — a shared, mutable cell whose writes
~        escape the closure and persist across calls.
~ There is no capture list and no marker: the operator that bound the name is the
~ signal. (Closures are monomorphic in M3 — concrete-typed params and captures.)

^ = () -> Num => <
  ~ `total` is `:=` -> captured by reference. `bump` mutates the shared cell, so
  ~ its writes accumulate across separate calls (they escape the closure).
  total := 0
  bump = n => <
    total := total + n
    total
  >

  bump(10)            ~ total -> 10
  bump(20)            ~ total -> 30  (the same cell, written again)

  ~ `base` is `=` -> captured by value. `addBase` sees a frozen copy; rebinding
  ~ `base` afterwards does NOT change what the closure already captured.
  base = 7
  addBase = x => x + base

  ~ 30 (accumulated via the :=-captured cell) + 12 (5 + the =-captured 7) = 42.
  total + addBase(5)
>
