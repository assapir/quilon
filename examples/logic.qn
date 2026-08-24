~ Logical operators — `&&` and `||` are SHORT-CIRCUIT: the right operand is
~ evaluated only when the left does not already decide the result. Every
~ assertion below holds, so the program exits 0.
<< core.test

^ = () -> $ => <
  ~ Count which right operands actually ran.
  hits := 0
  bump = (x :: Num) -> Bool => <
    hits := hits + x
    x > 0
  >

  a = false && bump(1)   ~ left decides (false) -> bump must NOT run
  b = true  || bump(2)   ~ left decides (true)  -> bump must NOT run
  c = true  && bump(4)   ~ undecided            -> bump runs
  d = false || bump(8)   ~ undecided            -> bump runs
  assertEq(hits, 12)
  assert(!a)
  assert(b)
  assert(c)
  assert(d)

  ~ The values are the ordinary truth table.
  assert(true && true)
  assert(!(true && false))
  assert(!(false && false))
  assert(true || false)
  assert(false || true)
  assert(!(false || false))

  ~ Short-circuit makes guarded indexing safe: with i out of range, the left
  ~ side is false and `xs[i]` is never evaluated.
  xs = [10, 20, 30]
  i = 5
  assert(!(i < xs.size && xs[i] == 10))
  $
>
