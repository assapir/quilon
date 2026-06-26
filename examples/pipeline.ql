~ The pipe `|>` feeds the left value in as the FIRST argument of the right call:
~   x |> f        is  f(x)
~   x |> f(a)     is  f(x, a)
~   a |> b |> c   is  c(b(a))
double  = (x :: Num) -> Num => x * 2
addFive = (x :: Num) -> Num => x + 5

^ = () -> Num => 10 |> double |> addFive   ~ addFive(double(10)) = 25
