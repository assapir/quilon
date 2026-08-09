~ Blocks `< ... >` evaluate to their last expression. Numbers are one unified `Num`.
~ `<< core.test` verifies the computed result; on success the program exits 0.
<< core.test

^ = () -> $ => <
  a :: Num = 5
  b :: Num = 7
  sum :: Num = a + b
  assertEq(sum, 12)
>
