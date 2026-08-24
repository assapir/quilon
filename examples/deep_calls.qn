~ Deep call chains: 40 nested calls and a 40-stage pipeline must CHECK fast —
~ argument re-inference at call sites was once exponential in nesting depth
~ (2^depth), which hung the checker on chains half this long.
<< core.test

g = (n :: Num) -> Num => n + 1

^ = () -> Num => <
  nested = g(g(g(g(g(g(g(g(g(g(g(g(g(g(g(g(g(g(g(g(g(g(g(g(g(g(g(g(g(g(g(g(g(g(g(g(g(g(g(g(1))))))))))))))))))))))))))))))))))))))))
  piped  = 1 |> g |> g |> g |> g |> g |> g |> g |> g |> g |> g |> g |> g |> g |> g |> g |> g |> g |> g |> g |> g |> g |> g |> g |> g |> g |> g |> g |> g |> g |> g |> g |> g |> g |> g |> g |> g |> g |> g |> g |> g 
  assertEq(nested, 41)
  assertEq(piped, 41)
  0
>
