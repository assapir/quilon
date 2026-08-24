~ Arithmetic — one unified `Num` (f64). Every expectation below is asserted,
~ so the program exits 0.
<< core.test

^ = () -> $ => <
  ~ The four basics on integers.
  assertEq(5 + 7, 12)
  assertEq(50 - 8, 42)
  assertEq(6 * 7, 42)
  assertEq(84 / 2, 42)

  ~ ...and on fractionals, including mixed integer/fractional operands.
  assertEq(1.5 + 2.25, 3.75)
  assertEq(5.5 - 1.25, 4.25)
  assertEq(2.5 * 4, 10)
  assertEq(42 + 3.14, 45.14)

  ~ Division is f64 division — it produces fractions, and the fraction keeps
  ~ working in further arithmetic.
  assertEq(7 / 2, 3.5)
  assertEq(1 / 4, 0.25)
  assertEq(7 / 2 + 7 / 2, 7)

  ~ Unary minus, including double negation.
  x = 5
  assertEq(-x, -5)
  assertEq(-(-x), 5)
  assertEq(-3 + 10, 7)

  ~ `%` is the f64 remainder: it works on fractional operands and the result
  ~ takes the DIVIDEND's sign (like C fmod / Rust %).
  assertEq(7 % 3, 1)
  assertEq(7.5 % 2, 1.5)
  assertEq(10 % 2.5, 0)
  assertEq((-7) % 3, -1)   ~ sign follows the dividend...
  assertEq(7 % (-3), 1)    ~ ...not the divisor

  ~ Precedence: `*` / `/` / `%` bind tighter than `+` / `-`; parentheses override.
  assertEq(2 + 3 * 4, 14)
  assertEq((2 + 3) * 4, 20)
  assertEq(10 - 6 / 2, 7)
  assertEq(20 % 7 - 2 * 3, 0)
  $
>
